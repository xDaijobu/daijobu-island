use super::medium::{self, Medium};
use super::{dropfiles, DragPayload};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use windows::core::{implement, w, Ref, Result, BOOL, HRESULT};
use windows::Win32::Foundation::{
    DATA_S_SAMEFORMATETC, DV_E_DVASPECT, DV_E_DVTARGETDEVICE, DV_E_FORMATETC, DV_E_LINDEX,
    DV_E_TYMED, E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, E_UNEXPECTED,
    OLE_E_ADVISENOTSUPPORTED, S_OK,
};
use windows::Win32::System::Com::{
    IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, DATADIR_GET,
    DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP};

#[derive(Clone)]
enum Data {
    Bytes(Arc<[u8]>),
    Private(Rc<Medium>),
}

struct Entry {
    format: FORMATETC,
    data: Data,
}

// Rc/RefCell and foreign private media are apartment-bound. The windows macro defaults
// to advertising IAgileObject/free-threaded marshaling, which would bypass the STA.
#[implement(IDataObject, Agile = false)]
struct DataObject {
    entries: RefCell<Vec<Entry>>,
    immutable_formats: Vec<u16>,
}

pub(super) fn format(id: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: id,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

pub(super) fn create(payload: &DragPayload) -> std::result::Result<IDataObject, String> {
    let mut entries = vec![Entry {
        format: format(CF_HDROP.0),
        data: Data::Bytes(dropfiles(&payload.paths)?.into()),
    }];
    if let Some(image) = payload.image.as_ref().filter(|_| payload.paths.len() == 1) {
        if image.png.is_empty() || image.dib.is_empty() {
            return Err("Drag image formats must not be empty".into());
        }
        // SAFETY: this only registers a format name; it neither opens nor changes the
        // clipboard. The constant string is NUL-terminated for the duration of the call.
        let png = unsafe { RegisterClipboardFormatW(w!("PNG")) };
        let png = u16::try_from(png)
            .ok()
            .filter(|id| *id != 0)
            .ok_or("Register drag PNG format failed")?;
        entries.extend([
            Entry {
                format: format(png),
                data: Data::Bytes(image.png.clone()),
            },
            Entry {
                format: format(CF_DIB.0),
                data: Data::Bytes(image.dib.clone()),
            },
        ]);
    }
    Ok(DataObject {
        immutable_formats: entries.iter().map(|entry| entry.format.cfFormat).collect(),
        entries: RefCell::new(entries),
    }
    .into())
}

fn validate_format(format: &FORMATETC) -> Result<()> {
    if format.cfFormat == 0 {
        return Err(DV_E_FORMATETC.into());
    }
    if !format.ptd.is_null() {
        return Err(DV_E_DVTARGETDEVICE.into());
    }
    if format.dwAspect != DVASPECT_CONTENT.0 {
        return Err(DV_E_DVASPECT.into());
    }
    if format.lindex != -1 {
        return Err(DV_E_LINDEX.into());
    }
    if format.tymed == 0 {
        return Err(DV_E_TYMED.into());
    }
    Ok(())
}

impl DataObject_Impl {
    fn find(&self, requested: &FORMATETC) -> Result<Data> {
        validate_format(requested)?;
        let entries = self.entries.try_borrow().map_err(|_| E_UNEXPECTED)?;
        let mut found_format = false;
        for entry in entries.iter() {
            if entry.format.cfFormat == requested.cfFormat {
                found_format = true;
                if entry.format.tymed & requested.tymed != 0 {
                    // Clone only Rust owners while borrowed. Native duplication/Release
                    // can invoke foreign code and must happen after this borrow ends.
                    return Ok(entry.data.clone());
                }
            }
        }
        Err(if found_format {
            DV_E_TYMED
        } else {
            DV_E_FORMATETC
        }
        .into())
    }
}

impl IDataObject_Impl for DataObject_Impl {
    fn GetData(&self, input: *const FORMATETC) -> Result<STGMEDIUM> {
        // SAFETY: COM guarantees readable non-null input pointers; explicitly reject null.
        let requested = unsafe { input.as_ref() }.ok_or(E_POINTER)?;
        let data = self.find(requested)?;
        let medium = match data {
            Data::Bytes(bytes) => Medium::bytes(&bytes)?,
            // SAFETY: the Rc keeps this validated, owned medium alive throughout copying.
            Data::Private(medium) => unsafe { Medium::duplicate(&medium.0, requested.cfFormat)? },
        };
        Ok(medium.into_raw())
    }

    fn GetDataHere(&self, _format: *const FORMATETC, _medium: *mut STGMEDIUM) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, input: *const FORMATETC) -> HRESULT {
        // SAFETY: see GetData; no handle allocation or clipboard access occurs.
        let Some(requested) = (unsafe { input.as_ref() }) else {
            return E_POINTER;
        };
        match self.find(requested) {
            Ok(_) => S_OK,
            Err(error) => error.code(),
        }
    }

    fn GetCanonicalFormatEtc(&self, input: *const FORMATETC, output: *mut FORMATETC) -> HRESULT {
        if output.is_null() {
            return E_POINTER;
        }
        // SAFETY: the caller provides a writable output; copy the input before writing
        // to allow the caller to reuse the same FORMATETC for both arguments.
        let requested = unsafe { input.as_ref() }.copied();
        // SAFETY: output is non-null and writable under the COM contract; ensure ptd is
        // NULL even on error, so there is never a borrowed device pointer to free.
        unsafe { output.write(FORMATETC::default()) };
        let Some(requested) = requested else {
            return E_POINTER;
        };
        if let Err(error) = self.find(&requested) {
            return error.code();
        }
        // SAFETY: successful validation requires requested.ptd == NULL.
        unsafe { output.write(requested) };
        DATA_S_SAMEFORMATETC
    }

    fn SetData(
        &self,
        input: *const FORMATETC,
        input_medium: *const STGMEDIUM,
        release: BOOL,
    ) -> Result<()> {
        // SAFETY: COM guarantees readable inputs when non-null; they remain borrowed
        // until every fallible validation/reservation has succeeded.
        let requested = unsafe { input.as_ref() }.ok_or(E_POINTER)?;
        // SAFETY: same COM input contract as above.
        let incoming = unsafe { input_medium.as_ref() }.ok_or(E_POINTER)?;
        validate_format(requested)?;
        if self.immutable_formats.contains(&requested.cfFormat) {
            return Err(DV_E_FORMATETC.into());
        }
        // SAFETY: incoming is a live COM-supplied medium. validate checks its union tag.
        unsafe { medium::validate(incoming)? };
        if requested.tymed & incoming.tymed == 0 {
            return Err(DV_E_TYMED.into());
        }
        // FALSE requires an independent owner before returning; no native call is made
        // while entries is borrowed (AddRef can call foreign code).
        let copy = if release.as_bool() {
            None
        } else {
            // SAFETY: incoming has just been validated and remains owned by the caller.
            Some(unsafe { Medium::duplicate(incoming, requested.cfFormat)? })
        };
        let mut entries = self.entries.try_borrow_mut().map_err(|_| E_UNEXPECTED)?;
        let existing = entries.iter().position(|entry| {
            entry.format.cfFormat == requested.cfFormat && entry.format.tymed == incoming.tymed
        });
        if existing.is_none() {
            entries.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
        }
        let owned = match copy {
            Some(copy) => copy,
            // SAFETY: fRelease TRUE transfers this exact medium, including its release
            // owner. All fallible work has passed; the caller no longer releases it.
            None => Medium(unsafe { std::ptr::read(incoming) }),
        };
        let entry = Entry {
            format: FORMATETC {
                tymed: incoming.tymed,
                ..*requested
            },
            data: Data::Private(Rc::new(owned)),
        };
        let replaced = if let Some(index) = existing {
            Some(std::mem::replace(&mut entries[index], entry))
        } else {
            entries.push(entry);
            None
        };
        drop(entries);
        // ReleaseStgMedium may call a custom release owner that reenters IDataObject.
        drop(replaced);
        Ok(())
    }

    fn EnumFormatEtc(&self, direction: u32) -> Result<IEnumFORMATETC> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        let formats = {
            let entries = self.entries.try_borrow().map_err(|_| E_UNEXPECTED)?;
            let mut formats: Vec<FORMATETC> = Vec::new();
            formats
                .try_reserve_exact(entries.len())
                .map_err(|_| E_OUTOFMEMORY)?;
            for entry in entries.iter() {
                if let Some(existing) = formats
                    .iter_mut()
                    .find(|f| f.cfFormat == entry.format.cfFormat)
                {
                    existing.tymed |= entry.format.tymed;
                } else {
                    formats.push(entry.format);
                }
            }
            formats
        };
        if formats.len() > u32::MAX as usize {
            return Err(E_INVALIDARG.into());
        }
        Ok(super::formats::enumerate(formats))
    }

    fn DAdvise(
        &self,
        _format: *const FORMATETC,
        _flags: u32,
        _sink: Ref<IAdviseSink>,
    ) -> Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
    fn DUnadvise(&self, _connection: u32) -> Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
    fn EnumDAdvise(&self) -> Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}
