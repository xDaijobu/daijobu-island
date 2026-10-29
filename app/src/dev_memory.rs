use crate::popups::PopupWin;
use dj_core::dev_memory::{bytes, visible_rows, Row};
use dj_win::dev_memory::{self, Sample};
use dj_win::popup::{self, PopupInput};
use dj_win::{window, HWND};
use std::sync::mpsc::{self, Receiver};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub const TIMER: usize = 30;
pub const WIDTH: f32 = 940.0;
pub const HEIGHT: f32 = 600.0;

#[derive(Default)]
pub struct Diagnostics {
    pub win: Option<PopupWin>,
    pending: Option<Receiver<Sample>>,
    cancel: Arc<AtomicBool>,
    last: Option<Instant>,
    sample: Option<(Instant, Sample)>,
    offset: usize,
    rows: Vec<Row>,
    error: Option<String>,
    pub logical: (f32, f32),
}

fn process_rows(sample: &Sample) -> Vec<Row> {
    let mut out = vec![Row::section(
        "Processes",
        "Verified app family · sorted by resident RAM, highest first",
    )];
    let mut processes: Vec<_> = sample.processes.iter().collect();
    processes.sort_by_key(|p| (std::cmp::Reverse(p.working_set), p.pid));
    for p in processes {
        let app = p.pid == std::process::id();
        let role = if app {
            "Main app: island, UI and feature controllers"
        } else {
            "Verified child process; component role not inferred"
        };
        out.push(Row {
            name: if app {
                format!("{} (this app)", p.name)
            } else {
                p.name.clone()
            },
            pid: Some(p.pid),
            memory: p.working_set,
            private_commit: p.private_commit,
            peak: p.peak_working_set,
            description: format!(
                "{role} · Private resident {}{}",
                bytes(p.private_working_set),
                if p.status.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", p.status)
                }
            ),
            ..Row::default()
        });
    }
    for (label, select) in [
        (
            "Resident RAM subtotal",
            (|p: &dev_memory::Process| p.working_set) as fn(&dev_memory::Process) -> Option<u64>,
        ),
        ("Private resident subtotal", |p: &dev_memory::Process| {
            p.private_working_set
        }),
        ("Private commit subtotal", |p: &dev_memory::Process| {
            p.private_commit
        }),
    ] {
        let values: Vec<_> = sample.processes.iter().filter_map(select).collect();
        out.push(Row::resource(label, bytes((!values.is_empty()).then(|| values.iter().sum())), format!("{}/{} processes readable; shared resident pages may count twice. Commit is not resident RAM.", values.len(), sample.processes.len())));
    }
    out.push(Row::resource(
        "Collection status",
        "Read-only",
        sample.note.clone(),
    ));
    out
}

impl Diagnostics {
    pub fn visible(&self) -> bool {
        self.win.as_ref().is_some_and(|w| w.shown)
    }

    pub fn request(&mut self, owner: HWND) {
        if !self.visible()
            || self.pending.is_some()
            || self
                .last
                .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&self.cancel);
        let (tx, rx) = mpsc::sync_channel(1);
        let raw = owner.0 as isize;
        match std::thread::Builder::new()
            .name("dj-dev-memory".into())
            .spawn(move || {
                let result = dev_memory::sample(&cancel);
                if !cancel.load(Ordering::Acquire) && tx.send(result).is_ok() {
                    window::wake(raw);
                }
            }) {
            Ok(_) => {
                self.pending = Some(rx);
                self.last = Some(Instant::now());
            }
            Err(e) => self.set_error(format!("Worker unavailable: {e}; R retries")),
        }
    }

    pub fn take(&mut self) -> bool {
        let result = self.pending.as_ref().map(|r| r.try_recv());
        match result {
            Some(Ok(sample)) => {
                self.pending = None;
                self.sample = Some((Instant::now(), sample));
                self.error = None;
                true
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.pending = None;
                self.set_error("Sampler disconnected; R retries".into());
                true
            }
            _ => false,
        }
    }

    fn set_error(&mut self, message: String) {
        self.error = Some(message);
        self.rows.retain(|r| r.name != "Sampler error");
        self.rows.insert(
            0,
            Row::resource(
                "Sampler error",
                "Unavailable",
                self.error.clone().unwrap_or_default(),
            ),
        );
        if let Some(win) = self.win.as_mut() {
            win.dirty = true;
        }
    }

    pub fn refresh_rows(&mut self, internal: Vec<Row>) {
        let mut rows = Vec::new();
        if let Some(error) = &self.error {
            rows.push(Row::resource("Sampler error", "Unavailable", error.clone()));
        }
        if let Some((at, sample)) = &self.sample {
            rows.extend(process_rows(sample));
            rows.push(Row::resource("Snapshot", format!("{:.1}s ago", at.elapsed().as_secs_f32()), "Processes sampled separately, not atomically. Peak is each process's highest working set."));
        } else {
            rows.push(Row::resource(
                "Processes",
                "Waiting",
                "Collecting process metadata on background worker.",
            ));
        }
        rows.push(Row::section("Internal resources", "Exact data lengths, estimates (~), or counts as labelled; overlapping CPU/GPU views, never additive."));
        rows.extend(internal);
        rows.push(Row::resource("Unaccounted memory", "Not measured", "Native heaps, stacks, DLLs, driver mappings and allocator slack; cannot subtract these overlapping counters from RAM."));
        rows.push(Row::resource("Privacy", "Metadata only", "No clipboard/terminal contents, titles or command lines collected. No export or automatic persistence."));
        self.rows = rows;
        self.clamp();
    }

    fn count(&self) -> usize {
        visible_rows(self.logical.1)
    }
    fn clamp(&mut self) {
        self.offset = self
            .offset
            .min(self.rows.len().saturating_sub(self.count()));
    }

    pub fn input(&mut self, input: PopupInput) -> bool {
        match input {
            PopupInput::Press { x, y }
                if self.win.as_ref().is_some_and(|w| {
                    let (x, y) = w.local(x, y);
                    x >= self.logical.0 - 110.0 && (53.0..83.0).contains(&y)
                }) =>
            {
                return true
            }
            PopupInput::Close | PopupInput::Key { vk: 0x1b, .. } => return true,
            PopupInput::Key { vk: 0x26, .. } => self.offset = self.offset.saturating_sub(1),
            PopupInput::Key { vk: 0x28, .. } => self.offset = self.offset.saturating_add(1),
            PopupInput::Key { vk: 0x21, .. } => {
                self.offset = self.offset.saturating_sub(self.count())
            }
            PopupInput::Key { vk: 0x22, .. } => {
                self.offset = self.offset.saturating_add(self.count())
            }
            PopupInput::Key { vk: 0x24, .. } => self.offset = 0,
            PopupInput::Key { vk: 0x23, .. } => self.offset = self.rows.len(),
            PopupInput::Wheel { lines, .. } => {
                let n = lines.abs().ceil().max(1.0) as usize;
                self.offset = if lines > 0.0 {
                    self.offset.saturating_sub(n)
                } else {
                    self.offset.saturating_add(n)
                };
            }
            PopupInput::Dpi { dpi } => {
                if let Some(win) = self.win.as_mut() {
                    win.set_dpi(dpi, self.logical.0, self.logical.1);
                }
            }
            _ => {}
        }
        self.clamp();
        if let Some(win) = self.win.as_mut() {
            win.dirty = true;
        }
        false
    }

    pub fn render(&mut self, theme: &dj_core::theme::ResolvedTokens) {
        let Some(win) = self.win.as_mut() else { return };
        if !win.dirty {
            return;
        }
        if let Some(r) = win.renderer.as_mut() {
            if let Err(e) = r.render_memory(&self.rows, self.offset, theme, win.scale, self.logical)
            {
                crate::storage::log(&format!("developer memory render: {e}"));
            }
        }
        win.dirty = false;
    }

    pub fn close(&mut self, owner: HWND) {
        self.cancel.store(true, Ordering::Release);
        window::kill_timer(owner, TIMER);
        if let Some(win) = self.win.take() {
            popup::hide(win.hwnd);
            win.destroy();
        }
        self.pending = None;
        self.sample = None;
        self.rows.clear();
        self.error = None;
        self.offset = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_is_not_zero_and_scroll_is_bounded() {
        assert_eq!(bytes(None), "Unavailable");
        assert_eq!(bytes(Some(0)), "0.00 KB");
        let mut d = Diagnostics {
            logical: (WIDTH, HEIGHT),
            rows: vec![Row::default(); 100],
            ..Diagnostics::default()
        };
        d.offset = usize::MAX;
        d.clamp();
        assert!(d.offset < 100);
        d.rows.clear();
        d.clamp();
        assert_eq!(d.offset, 0);
    }
    #[test]
    fn wheel_up_moves_back_and_errors_survive_refresh() {
        let mut d = Diagnostics {
            logical: (WIDTH, HEIGHT),
            rows: vec![Row::default(); 100],
            offset: 10,
            ..Diagnostics::default()
        };
        d.input(PopupInput::Wheel { lines: 1.0 });
        assert_eq!(d.offset, 9);
        d.input(PopupInput::Wheel { lines: -2.0 });
        assert_eq!(d.offset, 11);
        d.set_error("denied".into());
        d.refresh_rows(Vec::new());
        assert!(d
            .rows
            .iter()
            .any(|r| r.name == "Sampler error" && r.description == "denied"));
        let (tx, rx) = mpsc::sync_channel(1);
        d.pending = Some(rx);
        drop(tx);
        assert!(d.take());
        d.refresh_rows(Vec::new());
        assert!(d
            .rows
            .iter()
            .any(|r| r.description.contains("disconnected")));
    }

    #[test]
    fn process_rows_sort_memory_and_keep_unavailable_last() {
        let process = |pid, memory| dev_memory::Process {
            pid,
            parent: 0,
            name: "test.exe".into(),
            created: Some(1),
            working_set: memory,
            private_working_set: None,
            private_commit: None,
            peak_working_set: None,
            status: String::new(),
        };
        let rows = process_rows(&Sample {
            processes: vec![
                process(1, None),
                process(2, Some(10)),
                process(3, Some(100)),
            ],
            note: String::new(),
        });
        assert_eq!(
            rows.iter().filter_map(|r| r.pid).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn closed_never_dispatches_worker() {
        let mut d = Diagnostics::default();
        d.request(HWND::default());
        assert!(d.pending.is_none());
        assert!(d.last.is_none());
    }
}
