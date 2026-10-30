use std::sync::mpsc::{self, Receiver};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};

pub fn browse(owner: isize) -> Result<Receiver<Result<Option<String>, String>>, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("terminal-folder-picker".into())
        .spawn(move || {
            let result = pick(owner);
            let _ = tx.send(result);
            crate::window::wake(owner);
        })
        .map_err(|e| e.to_string())?;
    Ok(rx)
}

fn pick(owner: isize) -> Result<Option<String>, String> {
    // SAFETY: dedicated STA thread; COM objects and allocated path remain on this thread.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let result = (|| -> windows::core::Result<Option<String>> {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
            if let Err(e) = dialog.Show(Some(HWND(owner as _))) {
                if e.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) {
                    return Ok(None);
                }
                return Err(e);
            }
            let path = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let text = path.to_string();
            CoTaskMemFree(Some(path.0.cast()));
            Ok(Some(text.map_err(|_| {
                windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG)
            })?))
        })();
        CoUninitialize();
        result.map_err(|e| e.to_string())
    }
}
