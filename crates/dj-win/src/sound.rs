//! Plays an in-memory WAV with `PlaySound`, asynchronously.

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

/// Starts playing `wav` (a complete RIFF/WAVE file) and returns at once. A new call
/// cuts off the previous sound. False if Windows refused it. `wav` is `'static` because
/// an async `PlaySound` keeps reading the buffer after this returns.
pub fn play_wav(wav: &'static [u8]) -> bool {
    // SAFETY: with SND_MEMORY the first argument is a pointer to the WAV bytes, not a
    // string. The buffer is 'static, so it outlives the async playback.
    // SND_NODEFAULT keeps Windows from playing its default sound if the data is bad.
    unsafe {
        PlaySoundW(
            PCWSTR(wav.as_ptr().cast()),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        )
        .as_bool()
    }
}
