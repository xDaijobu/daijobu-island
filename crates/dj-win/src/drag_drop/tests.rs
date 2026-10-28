//! Synthetic COM/memory tests only: no desktop drag, clipboard access or OS input.

use super::data_object::{create, format};
use super::medium::Medium;
use super::*;
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStringExt;
use std::sync::atomic::AtomicUsize;
use windows::core::{IUnknown, Interface};
use windows::Win32::Foundation::{
    DATA_S_SAMEFORMATETC, DV_E_DVASPECT, DV_E_DVTARGETDEVICE, DV_E_FORMATETC, DV_E_LINDEX,
    DV_E_STGMEDIUM, DV_E_TYMED, E_FAIL, E_NOTIMPL, E_POINTER, HGLOBAL, RPC_E_CHANGED_MODE, S_FALSE,
};
use windows::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, IDataObject, COINIT_MULTITHREADED, DATADIR_GET, DATADIR_SET,
    DVASPECT_ICON, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_FILE, TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, DROPEFFECT_LINK, DROPEFFECT_MOVE};

fn payload(count: usize) -> DragPayload {
    DragPayload {
        paths: (0..count)
            .map(|n| PathBuf::from(format!(r"C:\synthetic\image-{n}.png")))
            .collect(),
        image: Some(ImageData {
            png: Arc::from(&b"synthetic png"[..]),
            dib: Arc::from(&b"synthetic dib"[..]),
        }),
        ghost: None,
    }
}

fn read(medium: &Medium) -> Vec<u8> {
    assert_eq!(medium.0.tymed, TYMED_HGLOBAL.0 as u32);
    // SAFETY: the wrapper owns a valid HGLOBAL. Read only its allocated extent while
    // locked, copy into Rust memory, then unlock before returning.
    unsafe {
        let handle = medium.0.u.hGlobal;
        let size = GlobalSize(handle);
        let ptr = GlobalLock(handle);
        assert!(!ptr.is_null());
        let out = std::slice::from_raw_parts(ptr.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(handle);
        out
    }
}

fn change_first_byte(medium: &Medium, value: u8) {
    // SAFETY: only test-owned nonempty HGLOBAL media are passed; the handle is locked.
    unsafe {
        let ptr = GlobalLock(medium.0.u.hGlobal).cast::<u8>();
        assert!(!ptr.is_null());
        ptr.write(value);
        let _ = GlobalUnlock(medium.0.u.hGlobal);
    }
}

fn get(object: &IDataObject, format: &FORMATETC) -> Medium {
    // SAFETY: object and format are live; the successful returned medium transfers to us.
    Medium(unsafe { object.GetData(format) }.expect("synthetic GetData"))
}

#[test]
fn unicode_dropfiles_preserves_order_header_and_double_terminator() {
    let paths = [
        PathBuf::from("C:\\画像\\猫🙂.png"),
        PathBuf::from(r"\\server\share\second image.png"),
    ];
    for count in 1..=paths.len() {
        let bytes = dropfiles(&paths[..count]).unwrap();
        assert_eq!(
            &bytes[..20],
            &[20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]
        );
        let units: Vec<u16> = bytes[20..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        let expected: Vec<u16> = paths[..count]
            .iter()
            .flat_map(|p| p.as_os_str().encode_wide().chain([0]))
            .chain([0])
            .collect();
        assert_eq!(units, expected);
        assert_eq!(&units[units.len() - 2..], &[0, 0]);
    }
}

#[test]
fn invalid_paths_and_size_overflows_are_rejected() {
    assert!(dropfiles(&[]).is_err());
    for path in [
        "",
        "relative.png",
        r"C:relative.png",
        r"\rooted.png",
        "C:\\bad\0name.png",
    ] {
        assert!(dropfiles(&[path.into()]).is_err(), "{path:?}");
    }
    assert!(dropfiles_size([usize::MAX]).is_err());
    assert!(dropfiles_size([usize::MAX / 2]).is_err());
    assert!(dropfiles_size([u32::MAX as usize]).is_err());
    assert!(dropfiles_size([usize::MAX - 10, 20]).is_err());
    // Windows filenames can contain unpaired UTF-16 surrogates; don't lossy-convert them.
    let path = PathBuf::from(std::ffi::OsString::from_wide(&[67, 58, 92, 0xD800]));
    assert_eq!(
        &dropfiles(&[path]).unwrap()[26..28],
        &0xD800u16.to_le_bytes()
    );
}

#[test]
fn single_image_formats_and_multi_file_exclusion_match_enumeration() {
    let _ole = OleApartment::initialize().unwrap();
    for count in [1, 3] {
        let data = payload(count);
        let object = create(&data).unwrap();
        // SAFETY: synthetic COM object and bounded writable enumeration buffer.
        unsafe {
            let enumerator = object.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap();
            let mut formats = [FORMATETC::default(); 8];
            let mut fetched = 0;
            assert_eq!(enumerator.Next(&mut formats, Some(&mut fetched)), S_FALSE);
            assert_eq!(fetched, if count == 1 { 3 } else { 1 });
            assert_eq!(formats[0].cfFormat, CF_HDROP.0);
            for format in &formats[..fetched as usize] {
                assert_eq!(object.QueryGetData(format), S_OK);
                assert!(format.ptd.is_null());
                assert!(!read(&get(&object, format)).is_empty());
            }
            assert_eq!(
                object.QueryGetData(&format(CF_DIB.0)),
                if count == 1 { S_OK } else { DV_E_FORMATETC }
            );
            assert_eq!(
                object
                    .EnumFormatEtc(DATADIR_SET.0 as u32)
                    .unwrap_err()
                    .code(),
                E_NOTIMPL
            );
        }
        assert_eq!(
            read(&get(&object, &format(CF_HDROP.0)))[..dropfiles(&data.paths).unwrap().len()],
            dropfiles(&data.paths).unwrap()
        );
    }
}

#[test]
fn formatetc_validation_is_consistent_between_query_and_get() {
    let object = create(&payload(1)).unwrap();
    let base = format(CF_HDROP.0);
    let cases = [
        (
            FORMATETC {
                cfFormat: 0,
                ..base
            },
            DV_E_FORMATETC,
        ),
        (
            FORMATETC {
                cfFormat: 12345,
                ..base
            },
            DV_E_FORMATETC,
        ),
        (
            FORMATETC {
                dwAspect: DVASPECT_ICON.0,
                ..base
            },
            DV_E_DVASPECT,
        ),
        (FORMATETC { lindex: 0, ..base }, DV_E_LINDEX),
        (FORMATETC { tymed: 0, ..base }, DV_E_TYMED),
        (
            FORMATETC {
                tymed: TYMED_FILE.0 as u32,
                ..base
            },
            DV_E_TYMED,
        ),
        (
            FORMATETC {
                ptd: std::ptr::dangling_mut(),
                ..base
            },
            DV_E_DVTARGETDEVICE,
        ),
    ];
    // SAFETY: synthetic input structs are readable; the deliberately non-null ptd is
    // rejected without dereferencing. All null parameters are deliberate contract tests.
    unsafe {
        for (format, expected) in cases {
            assert_eq!(object.QueryGetData(&format), expected);
            assert_eq!(object.GetData(&format).err().unwrap().code(), expected);
        }
        assert_eq!(object.QueryGetData(std::ptr::null()), E_POINTER);
        assert_eq!(
            object.GetData(std::ptr::null()).err().unwrap().code(),
            E_POINTER
        );
        let several = FORMATETC {
            tymed: (TYMED_HGLOBAL.0 | TYMED_ISTREAM.0) as u32,
            ..base
        };
        assert_eq!(object.QueryGetData(&several), S_OK);
        let mut canonical = FORMATETC::default();
        assert_eq!(
            object.GetCanonicalFormatEtc(&base, &mut canonical),
            DATA_S_SAMEFORMATETC
        );
        assert_eq!(canonical.cfFormat, base.cfFormat);
        assert!(canonical.ptd.is_null());
        assert_eq!(
            object.GetCanonicalFormatEtc(&base, std::ptr::null_mut()),
            E_POINTER
        );
    }
}

#[test]
fn getdata_media_are_independent_and_outlive_the_object() {
    let object = create(&payload(1)).unwrap();
    let one = get(&object, &format(CF_HDROP.0));
    let two = get(&object, &format(CF_HDROP.0));
    // SAFETY: both returned media are live HGLOBALs by the tested format contract.
    unsafe {
        assert_ne!(one.0.u.hGlobal, two.0.u.hGlobal);
    }
    let expected = read(&two);
    change_first_byte(&one, 0);
    assert_eq!(read(&two), expected);
    drop(one);
    drop(object);
    assert_eq!(read(&two), expected);
}

#[test]
fn data_object_stays_in_its_sta() {
    // RefCell/private media must never be exposed through the macro's default agile
    // marshaler: cross-thread COM calls must marshal back to the originating STA.
    let object = create(&payload(1)).unwrap();
    assert!(object
        .cast::<windows::Win32::System::Com::IAgileObject>()
        .is_err());
}

#[test]
fn borrowed_setdata_is_copied_and_replacement_keeps_old_readers_alive() {
    let object = create(&payload(1)).unwrap();
    let private = format(0xC123);
    let original = Medium::bytes(b"first private helper data").unwrap();
    // SAFETY: FALSE retains ownership of the valid caller-owned medium.
    unsafe { object.SetData(&private, &original.0, false) }.unwrap();
    change_first_byte(&original, b'X');
    drop(original);
    let old_reader = get(&object, &private);
    assert!(read(&old_reader).starts_with(b"first private helper data"));
    let replacement = Medium::bytes(b"replacement").unwrap();
    // SAFETY: borrowed valid medium, copied before the call returns.
    unsafe { object.SetData(&private, &replacement.0, false) }.unwrap();
    assert!(read(&get(&object, &private)).starts_with(b"replacement"));
    assert!(read(&old_reader).starts_with(b"first private helper data"));
    drop(object);
    assert!(read(&old_reader).starts_with(b"first private helper data"));
}

#[implement(IDropSource, Agile = false)]
struct ReleaseOwner {
    _backing: Medium,
    released: Arc<AtomicUsize>,
    on_release: Option<Box<dyn Fn()>>,
}
impl IDropSource_Impl for ReleaseOwner_Impl {
    fn QueryContinueDrag(&self, _escape: BOOL, _keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        DRAGDROP_S_CANCEL
    }
    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}
impl Drop for ReleaseOwner {
    fn drop(&mut self) {
        self.released.fetch_add(1, Ordering::SeqCst);
        if let Some(callback) = &self.on_release {
            callback();
        }
    }
}

fn custom_medium(released: Arc<AtomicUsize>, on_release: Option<Box<dyn Fn()>>) -> Medium {
    let backing = Medium::bytes(b"owned helper bytes").unwrap();
    // SAFETY: backing is a live HGLOBAL and its ownership is moved into the COM owner.
    let handle = unsafe { backing.0.u.hGlobal };
    let owner: IUnknown = ReleaseOwner {
        _backing: backing,
        released,
        on_release,
    }
    .into();
    Medium(STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: handle },
        pUnkForRelease: ManuallyDrop::new(Some(owner)),
    })
}

#[test]
fn transferred_setdata_honors_release_owner_once_and_allows_reentrant_release() {
    let object = create(&payload(1)).unwrap();
    let private = format(0xC124);
    let released = Arc::new(AtomicUsize::new(0));
    let reentered = Arc::new(AtomicBool::new(false));
    let probe = object.clone();
    let flag = reentered.clone();
    let medium = custom_medium(
        released.clone(),
        Some(Box::new(move || {
            // SAFETY: probe owns a live object. This callback runs during replacement's
            // ReleaseStgMedium, proving no RefCell borrow survived the release.
            assert_eq!(unsafe { probe.QueryGetData(&private) }, S_OK);
            flag.store(true, Ordering::SeqCst);
        })),
    );
    // SAFETY: successful TRUE transfers this medium; into_raw suppresses our destructor.
    unsafe { object.SetData(&private, &medium.0, true) }.unwrap();
    let _ = medium.into_raw();
    assert_eq!(released.load(Ordering::SeqCst), 0);
    let old_reader = get(&object, &private);
    let replacement = Medium::bytes(b"new data").unwrap();
    // SAFETY: borrowed replacement is copied and releases the prior custom owner.
    unsafe { object.SetData(&private, &replacement.0, false) }.unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert!(reentered.load(Ordering::SeqCst));
    assert!(read(&old_reader).starts_with(b"owned helper bytes"));
    drop(object);
    drop(old_reader);
    assert_eq!(released.load(Ordering::SeqCst), 1);
}

#[test]
fn borrowed_custom_owner_is_not_stolen_and_transferred_owner_drops_with_object() {
    for transfer in [false, true] {
        let object = create(&payload(1)).unwrap();
        let released = Arc::new(AtomicUsize::new(0));
        let medium = custom_medium(released.clone(), None);
        // SAFETY: successful TRUE transfers ownership; FALSE duplicates it.
        unsafe { object.SetData(&format(0xC125), &medium.0, transfer) }.unwrap();
        if transfer {
            let _ = medium.into_raw();
        } else {
            drop(medium);
        }
        assert_eq!(released.load(Ordering::SeqCst), usize::from(!transfer));
        let reader = get(&object, &format(0xC125));
        drop(object);
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert!(read(&reader).starts_with(b"owned helper bytes"));
    }
}

#[test]
fn failed_setdata_keeps_caller_ownership_and_protects_file_payload() {
    let object = create(&payload(1)).unwrap();
    let released = Arc::new(AtomicUsize::new(0));
    let medium = custom_medium(released.clone(), None);
    // SAFETY: on failure, even TRUE leaves medium ownership with the caller.
    unsafe {
        assert_eq!(
            object
                .SetData(&format(CF_HDROP.0), &medium.0, true)
                .unwrap_err()
                .code(),
            DV_E_FORMATETC
        );
        let mismatch = FORMATETC {
            tymed: TYMED_ISTREAM.0 as u32,
            ..format(0xC126)
        };
        assert_eq!(
            object
                .SetData(&mismatch, &medium.0, true)
                .unwrap_err()
                .code(),
            DV_E_TYMED
        );
        assert_eq!(
            object
                .SetData(std::ptr::null(), &medium.0, true)
                .unwrap_err()
                .code(),
            E_POINTER
        );
        assert_eq!(
            object
                .SetData(&format(0xC126), std::ptr::null(), true)
                .unwrap_err()
                .code(),
            E_POINTER
        );
        let empty = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            ..Default::default()
        };
        assert_eq!(
            object
                .SetData(&format(0xC126), &empty, true)
                .unwrap_err()
                .code(),
            DV_E_STGMEDIUM
        );
        let file = STGMEDIUM {
            tymed: TYMED_FILE.0 as u32,
            ..Default::default()
        };
        let file_format = FORMATETC {
            tymed: TYMED_FILE.0 as u32,
            ..format(0xC126)
        };
        assert_eq!(
            object
                .SetData(&file_format, &file, true)
                .unwrap_err()
                .code(),
            DV_E_TYMED
        );
    }
    assert_eq!(released.load(Ordering::SeqCst), 0);
    drop(object);
    assert_eq!(released.load(Ordering::SeqCst), 0);
    drop(medium);
    assert_eq!(released.load(Ordering::SeqCst), 1);
}

#[test]
fn stream_private_medium_survives_caller_and_object_release() {
    let _ole = OleApartment::initialize().unwrap();
    let object = create(&payload(1)).unwrap();
    let private = FORMATETC {
        tymed: TYMED_ISTREAM.0 as u32,
        ..format(0xC127)
    };
    // SAFETY: null HGLOBAL asks OLE to allocate a fresh stream, deleted on final release.
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }.unwrap();
    let input = Medium(STGMEDIUM {
        tymed: TYMED_ISTREAM.0 as u32,
        u: STGMEDIUM_0 {
            pstm: ManuallyDrop::new(Some(stream)),
        },
        ..Default::default()
    });
    // SAFETY: FALSE takes its own stream reference; no clipboard or file is involved.
    unsafe { object.SetData(&private, &input.0, false) }.unwrap();
    drop(input);
    let output = get(&object, &private);
    drop(object);
    // SAFETY: output owns the returned stream reference even after both other owners die.
    let handle = unsafe { GetHGlobalFromStream(output.0.u.pstm.as_ref().unwrap()) }.unwrap();
    assert!(!handle.is_invalid());
}

#[test]
fn one_private_format_can_offer_multiple_media_and_replace_each_independently() {
    let _ole = OleApartment::initialize().unwrap();
    let object = create(&payload(2)).unwrap();
    let global_format = format(0xC129);
    let stream_format = FORMATETC {
        tymed: TYMED_ISTREAM.0 as u32,
        ..global_format
    };
    let both = FORMATETC {
        tymed: global_format.tymed | stream_format.tymed,
        ..global_format
    };
    let first = Medium::bytes(b"original global").unwrap();
    // SAFETY: synthetic OLE memory stream; no file, clipboard or drag loop is used.
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }.unwrap();
    let stream_medium = Medium(STGMEDIUM {
        tymed: stream_format.tymed,
        u: STGMEDIUM_0 {
            pstm: ManuallyDrop::new(Some(stream)),
        },
        ..Default::default()
    });
    // SAFETY: FALSE copies the borrowed HGLOBAL; successful TRUE takes the stream.
    unsafe {
        object.SetData(&global_format, &first.0, false).unwrap();
        object.SetData(&both, &stream_medium.0, true).unwrap();
    }
    let _ = stream_medium.into_raw();
    drop(first);
    let old_stream = get(&object, &stream_format);
    let replacement = Medium::bytes(b"new global").unwrap();
    // SAFETY: borrowed valid replacement and bounded writable enumeration buffer.
    unsafe {
        object.SetData(&both, &replacement.0, false).unwrap();
        assert_eq!(object.QueryGetData(&both), S_OK);
        let enumerator = object.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap();
        let mut formats = [FORMATETC::default(); 8];
        let mut fetched = 0;
        assert_eq!(enumerator.Next(&mut formats, Some(&mut fetched)), S_FALSE);
        // One entry per clipboard format, with the complete mask of offered media.
        assert_eq!(fetched, 2);
        assert_eq!(formats[1].cfFormat, global_format.cfFormat);
        assert_eq!(formats[1].tymed, both.tymed);
    }
    assert!(read(&get(&object, &global_format)).starts_with(b"new global"));
    let new_stream = get(&object, &stream_format);
    drop(object);
    // SAFETY: both independent stream references still own the same live stream.
    unsafe {
        let old = GetHGlobalFromStream(old_stream.0.u.pstm.as_ref().unwrap()).unwrap();
        let new = GetHGlobalFromStream(new_stream.0.u.pstm.as_ref().unwrap()).unwrap();
        assert_eq!(old, new);
        assert!(!old.is_invalid());
    }
}

#[test]
fn stream_duplicates_balance_both_interface_and_custom_release_owner() {
    let _ole = OleApartment::initialize().unwrap();
    for transfer in [false, true] {
        let object = create(&payload(2)).unwrap();
        let private = FORMATETC {
            tymed: TYMED_ISTREAM.0 as u32,
            ..format(0xC131)
        };
        let released = Arc::new(AtomicUsize::new(0));
        // SAFETY: fresh OLE memory stream, with its own independent interface owner.
        let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }.unwrap();
        let owner: IUnknown = ReleaseOwner {
            _backing: Medium::bytes(b"custom release owner backing").unwrap(),
            released: released.clone(),
            on_release: None,
        }
        .into();
        let input = Medium(STGMEDIUM {
            tymed: private.tymed,
            u: STGMEDIUM_0 {
                pstm: ManuallyDrop::new(Some(stream)),
            },
            pUnkForRelease: ManuallyDrop::new(Some(owner)),
        });
        // SAFETY: TRUE transfers both interface references; FALSE AddRefs both.
        unsafe { object.SetData(&private, &input.0, transfer) }.unwrap();
        if transfer {
            let _ = input.into_raw();
        } else {
            drop(input);
        }
        assert_eq!(released.load(Ordering::SeqCst), 0);
        let output = get(&object, &private);
        drop(object);
        // Both returned interface and pUnkForRelease remain independently owned.
        assert_eq!(released.load(Ordering::SeqCst), 0);
        // SAFETY: output still owns a live stream after caller and data object die.
        let handle = unsafe { GetHGlobalFromStream(output.0.u.pstm.as_ref().unwrap()) }.unwrap();
        assert!(!handle.is_invalid());
        drop(output);
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn medium_requires_an_exact_discriminator_before_accepting_ownership() {
    let object = create(&payload(2)).unwrap();
    let private = FORMATETC {
        tymed: (TYMED_HGLOBAL.0 | TYMED_ISTREAM.0) as u32,
        ..format(0xC130)
    };
    let released = Arc::new(AtomicUsize::new(0));
    let mut incoming = custom_medium(released.clone(), None);
    incoming.0.tymed = private.tymed;
    // SAFETY: invalid combined tag is rejected before inspecting any union arm or
    // taking ownership; restore the real tag before our owner releases the medium.
    let error = unsafe { object.SetData(&private, &incoming.0, true) }.unwrap_err();
    incoming.0.tymed = TYMED_HGLOBAL.0 as u32;
    assert_eq!(error.code(), DV_E_TYMED);
    assert_eq!(released.load(Ordering::SeqCst), 0);
    // SAFETY: valid readable FORMATETC, and no rejected medium was installed.
    assert_eq!(unsafe { object.QueryGetData(&private) }, DV_E_FORMATETC);
    drop(object);
    drop(incoming);
    assert_eq!(released.load(Ordering::SeqCst), 1);
}

#[test]
fn empty_single_image_formats_fail_without_affecting_file_only_or_multi_file_drags() {
    for empty_png in [false, true] {
        let mut one = payload(1);
        let image = one.image.as_mut().unwrap();
        if empty_png {
            image.png = Arc::from([]);
        } else {
            image.dib = Arc::from([]);
        }
        assert!(create(&one).is_err());
        one.paths.push(PathBuf::from(r"C:\synthetic\second.png"));
        assert!(create(&one).is_ok());
    }
    let mut one = payload(1);
    one.image = None;
    assert!(create(&one).is_ok());
}

#[test]
fn enumerators_are_snapshots_with_independent_cursors() {
    let _ole = OleApartment::initialize().unwrap();
    let object = create(&payload(1)).unwrap();
    // SAFETY: native enumeration over synthetic objects and stack-owned output buffers.
    unsafe {
        let initial = object.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap();
        let mut one = [FORMATETC::default(); 1];
        assert_eq!(initial.Next(&mut one, None), S_OK);
        assert_eq!(one[0].cfFormat, CF_HDROP.0);
        let cloned = initial.Clone().unwrap();
        assert_eq!(initial.Next(&mut one, None), S_OK);
        let next = one[0].cfFormat;
        assert_eq!(cloned.Next(&mut one, None), S_OK);
        assert_eq!(one[0].cfFormat, next);
        initial.Reset().unwrap();
        assert_eq!(initial.Next(&mut one, None), S_OK);
        assert_eq!(one[0].cfFormat, CF_HDROP.0);
        let medium = Medium::bytes(b"extra").unwrap();
        object.SetData(&format(0xC128), &medium.0, false).unwrap();
        initial.Reset().unwrap();
        let mut all = [FORMATETC::default(); 8];
        let mut fetched = 0;
        assert_eq!(initial.Next(&mut all, Some(&mut fetched)), S_FALSE);
        assert_eq!(fetched, 3); // snapshot excludes subsequent SetData
        let current = object.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap();
        assert_eq!(current.Next(&mut all, Some(&mut fetched)), S_FALSE);
        assert_eq!(fetched, 4);
        current.Reset().unwrap();
        assert_eq!((current.vtable().Skip)(current.as_raw(), 4), S_OK);
        assert_eq!((current.vtable().Skip)(current.as_raw(), 1), S_FALSE);
    }
}

#[test]
fn enumerator_handles_null_outputs_partial_reads_and_exhaustion_without_advancing_on_error() {
    let object = create(&payload(2)).unwrap();
    // SAFETY: direct synthetic COM calls supply valid output buffers or deliberate
    // nulls that the enumerator rejects before dereferencing; no native drag is run.
    unsafe {
        let enumerator = object.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap();
        let mut fetched = 99;
        assert_eq!(
            (enumerator.vtable().Next)(enumerator.as_raw(), 1, std::ptr::null_mut(), &mut fetched),
            E_POINTER
        );
        assert_eq!(fetched, 0);
        let mut outputs = [FORMATETC::default(); 2];
        assert_eq!(enumerator.Next(&mut outputs, None), E_POINTER);
        assert_eq!(enumerator.Next(&mut outputs, Some(&mut fetched)), S_FALSE);
        assert_eq!(fetched, 1);
        assert_eq!(outputs[0].cfFormat, CF_HDROP.0);
        let exhausted_clone = enumerator.Clone().unwrap();
        fetched = 99;
        assert_eq!(
            exhausted_clone.Next(&mut outputs, Some(&mut fetched)),
            S_FALSE
        );
        assert_eq!(fetched, 0);
        assert_eq!(
            (enumerator.vtable().Next)(
                enumerator.as_raw(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            S_OK
        );
        enumerator.Reset().unwrap();
        assert_eq!((enumerator.vtable().Skip)(enumerator.as_raw(), 0), S_OK);
        assert_eq!(
            (enumerator.vtable().Skip)(enumerator.as_raw(), u32::MAX),
            S_FALSE
        );
        assert_eq!(enumerator.Next(&mut outputs, Some(&mut fetched)), S_FALSE);
        assert_eq!(fetched, 0);
    }
}

#[test]
fn source_cancels_escape_external_events_and_right_button() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let source: IDropSource = DropSource {
        cancelled: cancelled.clone(),
    }
    .into();
    // SAFETY: direct synthetic COM callbacks, without starting a modal loop or input.
    unsafe {
        assert_eq!(source.QueryContinueDrag(false, MK_LBUTTON), S_OK);
        assert_eq!(
            source.QueryContinueDrag(false, MODIFIERKEYS_FLAGS(0)),
            DRAGDROP_S_DROP
        );
        assert_eq!(
            source.QueryContinueDrag(true, MK_LBUTTON),
            DRAGDROP_S_CANCEL
        );
        assert_eq!(
            source.QueryContinueDrag(false, MK_LBUTTON | MK_RBUTTON),
            DRAGDROP_S_CANCEL
        );
        for event in ["hide", "quit", "capture", "display change"] {
            cancelled.store(true, Ordering::Release);
            assert_eq!(
                source.QueryContinueDrag(false, MK_LBUTTON),
                DRAGDROP_S_CANCEL,
                "{event}"
            );
            assert_eq!(
                source.QueryContinueDrag(false, MODIFIERKEYS_FLAGS(0)),
                DRAGDROP_S_CANCEL
            );
            cancelled.store(false, Ordering::Release);
        }
        assert_eq!(
            source.GiveFeedback(DROPEFFECT_COPY),
            DRAGDROP_S_USEDEFAULTCURSORS
        );
    }
}

#[test]
fn only_confirmed_copy_is_success() {
    assert_eq!(
        map_result(DRAGDROP_S_DROP, DROPEFFECT_COPY),
        Ok(DragOutcome::DroppedCopy)
    );
    for result in [S_OK, S_FALSE, DRAGDROP_S_CANCEL] {
        assert_eq!(
            map_result(result, DROPEFFECT_COPY),
            Ok(DragOutcome::Cancelled)
        );
    }
    for effect in [
        DROPEFFECT_NONE,
        DROPEFFECT_MOVE,
        DROPEFFECT_LINK,
        DROPEFFECT_COPY | DROPEFFECT_MOVE,
    ] {
        assert_eq!(
            map_result(DRAGDROP_S_DROP, effect),
            Ok(DragOutcome::Cancelled)
        );
    }
    assert!(map_result(E_FAIL, DROPEFFECT_COPY).is_err());
    // An already cancelled request exits before touching OLE or validating files.
    assert_eq!(
        run(payload(0), Arc::new(AtomicBool::new(true))),
        Ok(DragOutcome::Cancelled)
    );
}

#[test]
fn final_preflight_cancels_input_changes_during_helper_setup() {
    for cancelled in [false, true] {
        for primary_down in [false, true] {
            for escape_down in [false, true] {
                assert_eq!(
                    preflight(cancelled, primary_down, escape_down),
                    !cancelled && primary_down && !escape_down
                );
            }
        }
    }
    assert!(preflight(false, true, false));
    // A request was valid before setup; release, Escape and a deferred event all
    // invalidate it before the modal call, independently of the old readiness decision.
    assert!(!preflight(false, false, false));
    assert!(!preflight(false, true, true));
    // A quick Escape tap during COM setup has already been released; the request
    // watcher's latched cancellation still prevents entering OLE.
    assert!(!preflight(true, true, false));
}

#[test]
fn ghost_dimensions_bytes_and_count_are_checked_without_native_ui() {
    let base = Ghost {
        width: 2,
        height: 3,
        bgra: vec![255; 24].into(),
        count: 3,
    };
    assert!(ghost::validate(&base, 3).is_ok());
    for width in [0, 513, u32::MAX] {
        assert!(ghost::validate(
            &Ghost {
                width,
                ..base.clone()
            },
            3
        )
        .is_err());
    }
    assert!(ghost::validate(
        &Ghost {
            height: 0,
            ..base.clone()
        },
        3
    )
    .is_err());
    assert!(ghost::validate(
        &Ghost {
            bgra: vec![0; 23].into(),
            ..base.clone()
        },
        3
    )
    .is_err());
    assert!(ghost::validate(&base, 1).is_err());
    assert!(ghost::validate(&Ghost { count: 0, ..base }, 0).is_err());
}

#[test]
fn ole_apartment_rejects_mta_and_balances_nested_initialization() {
    std::thread::spawn(|| {
        // SAFETY: this fresh test thread exclusively balances all its COM initialization;
        // it creates no UI and does not enter a drag loop.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            assert_eq!(
                OleApartment::initialize().err().unwrap().code(),
                RPC_E_CHANGED_MODE
            );
            CoUninitialize();
            {
                let _one = OleApartment::initialize().unwrap();
                let _two = OleApartment::initialize().unwrap();
                assert_eq!(
                    CoInitializeEx(None, COINIT_MULTITHREADED),
                    RPC_E_CHANGED_MODE
                );
            }
            // Success proves both the S_OK and S_FALSE OleInitialize calls were balanced.
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            CoUninitialize();
        }
    })
    .join()
    .unwrap();
}
