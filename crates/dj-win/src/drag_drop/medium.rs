//! RAII for OLE media. STGMEDIUM's generated Clone is a shallow bit copy, not ownership.

use std::mem::ManuallyDrop;
use windows::core::{Error, Result, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, DV_E_STGMEDIUM, DV_E_TYMED, E_OUTOFMEMORY, HANDLE};
use windows::Win32::Graphics::Gdi::CopyEnhMetaFileW;
use windows::Win32::System::Com::{
    STGMEDIUM, STGMEDIUM_0, TYMED_ENHMF, TYMED_GDI, TYMED_HGLOBAL, TYMED_ISTORAGE, TYMED_ISTREAM,
    TYMED_MFPICT,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::System::Ole::{
    OleDuplicateData, ReleaseStgMedium, CF_BITMAP, CF_METAFILEPICT, CF_PALETTE,
};

pub(super) struct Medium(pub(super) STGMEDIUM);

impl Drop for Medium {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the medium, including its pUnkForRelease
        // reference if present. ReleaseStgMedium selects the matching destructor.
        unsafe { ReleaseStgMedium(&mut self.0) };
    }
}

impl Medium {
    pub(super) fn into_raw(self) -> STGMEDIUM {
        let this = ManuallyDrop::new(self);
        // SAFETY: transfer the unique ownership without running the wrapper destructor.
        unsafe { std::ptr::read(&this.0) }
    }

    pub(super) fn bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > isize::MAX as usize {
            return Err(DV_E_STGMEDIUM.into());
        }
        // SAFETY: allocate and lock exactly enough writable memory. On either failure
        // the allocation is freed; success transfers it to the STGMEDIUM wrapper.
        unsafe {
            // Zero allocation padding too: receivers may inspect all of GlobalSize.
            let handle = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len())?;
            let data = GlobalLock(handle);
            if data.is_null() {
                let _ = GlobalFree(Some(handle));
                return Err(Error::from_hresult(E_OUTOFMEMORY));
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data.cast(), bytes.len());
            let _ = GlobalUnlock(handle);
            Ok(Self(STGMEDIUM {
                tymed: TYMED_HGLOBAL.0 as u32,
                u: STGMEDIUM_0 { hGlobal: handle },
                pUnkForRelease: ManuallyDrop::new(None),
            }))
        }
    }

    /// Copies handles, or AddRefs stream/storage interfaces. Returned HGLOBALs never
    /// alias the source; a receiver may release/mutate one without affecting later reads.
    ///
    /// # Safety
    /// `source` must contain a live medium with the union arm specified by `tymed`.
    pub(super) unsafe fn duplicate(source: &STGMEDIUM, format: u16) -> Result<Self> {
        // SAFETY: the caller guarantees valid handles/interfaces; validate() checks the
        // discriminator and non-null arm before we access it. All returned owners are new.
        unsafe {
            validate(source)?;
            let mut out = STGMEDIUM {
                tymed: source.tymed,
                ..Default::default()
            };
            match source.tymed {
                t if t == TYMED_HGLOBAL.0 as u32 => {
                    let handle = source.u.hGlobal;
                    let size = GlobalSize(handle);
                    if size == 0 || size > isize::MAX as usize {
                        return Err(DV_E_STGMEDIUM.into());
                    }
                    let data = GlobalLock(handle);
                    if data.is_null() {
                        return Err(DV_E_STGMEDIUM.into());
                    }
                    let copy = Self::bytes(std::slice::from_raw_parts(data.cast(), size));
                    let _ = GlobalUnlock(handle);
                    return copy;
                }
                t if t == TYMED_ISTREAM.0 as u32 => {
                    out.u.pstm = ManuallyDrop::new(source.u.pstm.as_ref().cloned());
                    out.pUnkForRelease = ManuallyDrop::new(source.pUnkForRelease.as_ref().cloned());
                }
                t if t == TYMED_ISTORAGE.0 as u32 => {
                    out.u.pstg = ManuallyDrop::new(source.u.pstg.as_ref().cloned());
                    out.pUnkForRelease = ManuallyDrop::new(source.pUnkForRelease.as_ref().cloned());
                }
                t if t == TYMED_ENHMF.0 as u32 => {
                    let copy = CopyEnhMetaFileW(source.u.hEnhMetaFile, PCWSTR::null());
                    if copy.is_invalid() {
                        return Err(E_OUTOFMEMORY.into());
                    }
                    out.u.hEnhMetaFile = copy;
                }
                t if t == TYMED_MFPICT.0 as u32 => {
                    let copy = OleDuplicateData(
                        HANDLE(source.u.hMetaFilePict),
                        CF_METAFILEPICT,
                        GMEM_MOVEABLE,
                    );
                    if copy.is_invalid() {
                        return Err(E_OUTOFMEMORY.into());
                    }
                    out.u.hMetaFilePict = copy.0;
                }
                t if t == TYMED_GDI.0 as u32 => {
                    let kind = if format == CF_PALETTE.0 {
                        CF_PALETTE
                    } else {
                        CF_BITMAP
                    };
                    let copy = OleDuplicateData(HANDLE(source.u.hBitmap.0), kind, GMEM_MOVEABLE);
                    if copy.is_invalid() {
                        return Err(E_OUTOFMEMORY.into());
                    }
                    out.u.hBitmap = windows::Win32::Graphics::Gdi::HBITMAP(copy.0);
                }
                _ => return Err(DV_E_TYMED.into()),
            }
            Ok(Self(out))
        }
    }
}

/// # Safety
/// Non-null handles/interfaces must be valid according to the COM call contract.
pub(super) unsafe fn validate(medium: &STGMEDIUM) -> Result<()> {
    // SAFETY: inspect only the union arm corresponding to the exact discriminator.
    let present = unsafe {
        match medium.tymed {
            t if t == TYMED_HGLOBAL.0 as u32 => !medium.u.hGlobal.is_invalid(),
            t if t == TYMED_GDI.0 as u32 => !medium.u.hBitmap.is_invalid(),
            t if t == TYMED_ENHMF.0 as u32 => !medium.u.hEnhMetaFile.is_invalid(),
            t if t == TYMED_MFPICT.0 as u32 => !medium.u.hMetaFilePict.is_null(),
            t if t == TYMED_ISTREAM.0 as u32 => medium.u.pstm.is_some(),
            t if t == TYMED_ISTORAGE.0 as u32 => medium.u.pstg.is_some(),
            // TYMED_FILE could delete the named file in ReleaseStgMedium. It is never
            // accepted. File transfer uses immutable CF_HDROP bytes instead.
            _ => return Err(DV_E_TYMED.into()),
        }
    };
    if present {
        Ok(())
    } else {
        Err(DV_E_STGMEDIUM.into())
    }
}
