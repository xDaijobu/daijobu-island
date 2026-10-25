//! Cover art download. Runs on a worker thread; the bytes go back to the UI thread
//! through a channel, and the UI loop is woken with `window::wake`. Decoding happens
//! on the UI thread in the renderer.

use dj_win::window;
use std::sync::mpsc::Sender;

/// Largest artwork accepted. Covers are 60–544 px, so this is generous.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// One finished download. `bytes` is `None` when the request failed or was too large.
pub struct ArtResult {
    pub url: String,
    pub bytes: Option<Vec<u8>>,
}

/// Starts a download for `url`. The result is sent on `tx`, then the window `hwnd`
/// is woken so the UI thread drains the channel.
pub fn fetch(url: String, hwnd: isize, tx: Sender<ArtResult>) {
    std::thread::spawn(move || {
        let bytes = download(&url);
        // A closed channel means the app is shutting down; nothing to do.
        if tx.send(ArtResult { url, bytes }).is_ok() {
            window::wake(hwnd);
        }
    });
}

fn download(url: &str) -> Option<Vec<u8>> {
    let mut resp = ureq::get(url).call().ok()?;
    resp.body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .ok()
        .filter(|b| !b.is_empty())
}
