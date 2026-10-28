//! Wheel routing follows the program's VT modes. Mouse-tracking programs own their
//! scroll; ordinary shells use the emulator's history instead.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Term, TermMode};

/// Scroll the history or return bytes for the PTY. Coordinates are zero-based cells.
pub(crate) fn route<T: EventListener>(
    term: &mut Term<T>,
    lines: i32,
    col: usize,
    row: usize,
) -> Vec<u8> {
    if lines == 0 {
        return Vec::new();
    }
    let mode = *term.mode();
    let mut bytes = Vec::new();
    // Match the emulator's 500-row grid bound; a malformed wheel message must not
    // allocate or enqueue millions of reports on the UI thread.
    let count = lines.unsigned_abs().min(500);
    if mode.intersects(TermMode::MOUSE_MODE) {
        let button = if lines > 0 { 64 } else { 65 };
        let col = col.min(term.columns() - 1);
        let row = row.min(term.screen_lines() - 1);
        let report = if mode.contains(TermMode::SGR_MOUSE) {
            format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes()
        } else {
            // Legacy coordinates are single bytes; UTF-8 mode extends their range.
            if !mode.contains(TermMode::UTF8_MOUSE) && (col >= 223 || row >= 223) {
                return bytes;
            }
            let mut report = vec![0x1b, b'[', b'M', button + 32];
            for value in [col + 33, row + 33] {
                if mode.contains(TermMode::UTF8_MOUSE) {
                    let mut utf8 = [0; 4];
                    report.extend_from_slice(
                        char::from_u32(value as u32)
                            .unwrap()
                            .encode_utf8(&mut utf8)
                            .as_bytes(),
                    );
                } else {
                    report.push(value as u8);
                }
            }
            report
        };
        for _ in 0..count {
            bytes.extend_from_slice(&report);
        }
    } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        let prefix = if mode.contains(TermMode::APP_CURSOR) {
            b'O'
        } else {
            b'['
        };
        let arrow = if lines > 0 { b'A' } else { b'B' };
        for _ in 0..count {
            bytes.extend_from_slice(&[0x1b, prefix, arrow]);
        }
    } else {
        term.scroll_display(Scroll::Delta(lines));
    }
    bytes
}
