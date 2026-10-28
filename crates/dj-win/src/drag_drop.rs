//! Owned, copy-only OLE drag transport. No clipboard publication or Snap state lives here.
//!
//! Call [`run`] on the UI STA only after releasing application borrows and pointer capture:
//! `DoDragDrop` pumps a reentrant message loop. The caller may set `cancelled` from native
//! dispatch, but must defer disruptive application work until this function returns.
//! The token is checked in `QueryContinueDrag`; the caller must also wake/cancel OLE's
//! current-thread capture when cancelling without a mouse or keyboard state change.

mod data_object;
mod formats;
mod ghost;
mod medium;
#[cfg(test)]
mod tests;

use std::marker::PhantomData;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::{implement, BOOL, HRESULT};
use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK,
};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, OleUninitialize, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};

#[derive(Clone, Debug)]
pub struct ImageData {
    pub png: Arc<[u8]>,
    /// A CF_DIB block, including its bitmap header (not a BMP file header).
    pub dib: Arc<[u8]>,
}

#[derive(Clone, Debug)]
pub struct Ghost {
    pub width: u32,
    pub height: u32,
    /// Top-down, tightly packed BGRA with straight (not premultiplied) alpha.
    pub bgra: Arc<[u8]>,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct DragPayload {
    pub paths: Vec<PathBuf>,
    /// Offered only when `paths` contains exactly one entry.
    pub image: Option<ImageData>,
    pub ghost: Option<Ghost>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragOutcome {
    DroppedCopy,
    Cancelled,
}

/// Runs an OLE drag with owned inputs. Files are never modified or deleted here.
/// A rejected drop is cancellation; native transport errors are returned to the caller.
pub fn run(payload: DragPayload, cancelled: Arc<AtomicBool>) -> Result<DragOutcome, String> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(DragOutcome::Cancelled);
    }
    let _ole = OleApartment::initialize().map_err(|e| format!("Initialize drag OLE: {e}"))?;
    let object = data_object::create(&payload)?;
    // The helper is optional. Its COM object stays alive through the modal loop.
    let _helper = payload.ghost.as_ref().and_then(|image| {
        match ghost::initialize(&object, image, payload.paths.len()) {
            Ok(helper) => Some(helper),
            Err(error) => {
                eprintln!("Daijobu Island: drag image unavailable; using standard cursor: {error}");
                None
            }
        }
    });
    let source: IDropSource = DropSource {
        cancelled: cancelled.clone(),
    }
    .into();
    let mut effect = DROPEFFECT_NONE;
    // Helper creation may pump messages. Recheck physical input after all COM setup so
    // releasing during setup never initiates an unsolicited modal drag. Async key state
    // uses physical buttons, so honor the user's swapped-button setting explicitly.
    // SAFETY: read-only, current input/system metrics queries; no OS input is generated.
    let (primary_down, escape_down) = unsafe {
        let primary = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
            VK_RBUTTON
        } else {
            VK_LBUTTON
        };
        (
            GetAsyncKeyState(primary.0 as i32) < 0,
            GetAsyncKeyState(VK_ESCAPE.0 as i32) < 0,
        )
    };
    if !preflight(cancelled.load(Ordering::Acquire), primary_down, escape_down) {
        return Ok(DragOutcome::Cancelled);
    }
    // Keep the request-scoped Escape hook armed across all potentially reentrant COM
    // setup. Disarm only now: OLE takes over Escape detection in the very next call.
    crate::window::finish_drag_preparation();
    // SAFETY: OLE is initialized on this STA; both COM objects own all their data and
    // outlive the modal call. No application reference or RefCell borrow is held here.
    let result = unsafe { DoDragDrop(&object, &source, DROPEFFECT_COPY, &mut effect) };
    map_result(result, effect)
}

fn preflight(cancelled: bool, primary_down: bool, escape_down: bool) -> bool {
    !cancelled && primary_down && !escape_down
}

struct OleApartment(PhantomData<Rc<()>>);

impl OleApartment {
    fn initialize() -> windows::core::Result<Self> {
        // SAFETY: initializes the current thread. S_OK and S_FALSE both incur one
        // matching OleUninitialize; RPC_E_CHANGED_MODE returns without constructing us.
        unsafe { OleInitialize(None)? };
        Ok(Self(PhantomData))
    }
}

impl Drop for OleApartment {
    fn drop(&mut self) {
        // SAFETY: the !Send guard drops on the thread whose successful init it balances.
        unsafe { OleUninitialize() };
    }
}

#[implement(IDropSource)]
struct DropSource {
    cancelled: Arc<AtomicBool>,
}

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() || self.cancelled.load(Ordering::Acquire) || keys.contains(MK_RBUTTON) {
            DRAGDROP_S_CANCEL
        } else if !keys.contains(MK_LBUTTON) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

fn map_result(result: HRESULT, effect: DROPEFFECT) -> Result<DragOutcome, String> {
    if result.is_err() {
        Err(format!(
            "Drag failed: {}",
            windows::core::Error::from_hresult(result)
        ))
    } else if result == DRAGDROP_S_DROP && effect == DROPEFFECT_COPY {
        Ok(DragOutcome::DroppedCopy)
    } else {
        // Mouse release alone, NONE, MOVE, LINK and unexpected success statuses do not
        // prove a copy occurred. In particular, never run a source-file delete callback.
        Ok(DragOutcome::Cancelled)
    }
}

const DROPFILES_HEADER: usize = 20;

fn dropfiles_size(lengths: impl IntoIterator<Item = usize>) -> Result<usize, String> {
    let mut units = 1usize; // final NUL, in addition to each filename's NUL
    for length in lengths {
        units = units
            .checked_add(length)
            .and_then(|n| n.checked_add(1))
            .ok_or("Drag file list is too large")?;
    }
    units
        .checked_mul(2)
        .and_then(|n| n.checked_add(DROPFILES_HEADER))
        .filter(|n| *n <= isize::MAX as usize && *n <= u32::MAX as usize)
        .ok_or_else(|| "Drag file list is too large".into())
}

fn dropfiles(paths: &[PathBuf]) -> Result<Vec<u8>, String> {
    if paths.is_empty() {
        return Err("A drag needs at least one file".into());
    }
    for path in paths {
        if !path.is_absolute() || path.as_os_str().encode_wide().any(|u| u == 0) {
            return Err("Drag paths must be absolute and contain no NUL".into());
        }
    }
    let size = dropfiles_size(paths.iter().map(|p| p.as_os_str().encode_wide().count()))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| "Drag file list allocation failed")?;
    // DROPFILES: offset, POINT.x/y, fNC, fWide. Encode explicitly to avoid layout padding.
    for value in [DROPFILES_HEADER as u32, 0, 0, 0, 1] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for path in paths {
        for unit in path.as_os_str().encode_wide().chain(std::iter::once(0)) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&0u16.to_le_bytes());
    Ok(bytes)
}
