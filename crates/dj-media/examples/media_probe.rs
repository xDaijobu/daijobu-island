//! Read-only acceptance probe: never sends controls, enables lyrics, or registers packages.
use dj_media::Media;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

fn main() {
    let seconds = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(10)
        .clamp(1, 60);
    let mut length = 0;
    // SAFETY: documented size-only query; no output pointer is passed.
    let identity = unsafe { GetCurrentPackageFullName(&mut length, None) };
    println!(
        "read_only=true lyrics_enabled=false package_identity={} identity_query_code={}",
        length > 0,
        identity.0
    );
    let (tx, rx) = mpsc::sync_channel(1);
    let media = Media::start(false, move || {
        let _ = tx.try_send(());
    });
    let until = Instant::now() + Duration::from_secs(seconds);
    let mut updates = 0;
    while let Some(remaining) = until.checked_duration_since(Instant::now()) {
        if rx.recv_timeout(remaining).is_err() {
            break;
        }
        if let Some(update) = media.take() {
            updates += 1;
            if let Some(now) = &update.session.now {
                println!("generation={} session=true source={:?} playing={} title_present={} artist_present={} album_present={} duration_known={} position_finite={} art_bytes={} can_play={} can_pause={} can_previous={} can_next={} can_seek={} lyrics={:?}", update.session.target.0, now.source, now.playing, !now.title.is_empty(), !now.artist.is_empty(), !now.album.is_empty(), now.dur > 0.0, update.session.position().is_finite(), update.art.as_ref().map_or(0, |a| a.len()), now.controls.play, now.controls.pause, now.controls.previous, now.controls.next, now.controls.seek, update.lyrics.state);
            } else {
                println!(
                    "generation={} session=false error={:?}",
                    update.session.target.0, update.session.error
                );
            }
        }
    }
    drop(media);
    println!("updates={updates}; no playback commands or network requests sent");
    if updates == 0 {
        std::process::exit(1);
    }
}
