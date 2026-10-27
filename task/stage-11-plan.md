# Stage 11 dj CLI — implementation plan

> Written by the coordinator (Claude Opus 5.5). Parts A–D run in parallel (one Claude Sonnet 5.5 worker each, own git worktree); part F (wiring, docs, report) is the coordinator's. Each worker reads only its part plus "Global Constraints" and "Shared constants".

**Goal:** scripts talk to the island through a per-user named pipe; the `dj` binary sends `progress / done / notify / theme / snap`; the island shows a progress pill and a notice pill.

**Spec:** `task/progress.md` → "Stage 11: dj CLI".

## Global Constraints

- Rust stable (MSVC), `windows` 0.62, `serde` + `serde_json` (already in the workspace). **No new third-party dependencies** (no clap: parse arguments by hand). Apache-2.0.
- `dj-core` is `#![forbid(unsafe_code)]`, no Win32/Skia. `unsafe` only in `dj-ipc`, `dj-win`, `dj-render`, each block with `// SAFETY:`.
- User-facing text is English. The name is "Daijobu Island"; the binary is `dj`.
- Idle CPU ≈ 0: the pipe thread blocks; the island has no polling.
- Headless tests only: no exe, no windows, no hooks. The only pipe a test may create is an in-process one with a test-only name.
- Every Bash call: `source C:/Users/UseR/orca/workspaces/daijobu-island/dj-build-env.sh && export CARGO_TARGET_DIR=<your worktree>/target CARGO_BUILD_JOBS=4 && cd <your worktree> && …` (override the shared target).
- Workers do not commit. Gate: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p <your crate>`.

## Shared constants

- Protocol version 1; one JSON object per message, UTF-8, ending with `\n`; a frame is at most 4096 bytes.
- Label ≤ 48 chars, message ≤ 120 chars (counted in `char`s), after trimming, collapsing whitespace runs to one space and dropping control characters. An empty label is allowed for progress (shown as "working"); an empty message for done becomes "done"; an empty notify message is an error.
- Times: done shows 4 s, notice 5 s, a progress without updates expires after 30 min. The client waits 3 s for a reply; the server drops a silent client after 2 s.
- Island: progress pill 300 × 36 r 18; notice pill 360 × 36 r 18.
- App timer id `TIMER_DJ = 13` (1–12 are taken). Exit codes: 0 ok, 1 usage/island error, 2 not running.

---

## Part A — `dj_core::dj` (worktree `11-core`)

**Files:** create `crates/dj-core/src/dj.rs`; `crates/dj-core/src/lib.rs` (one `pub mod` line + doc). Nothing else. (The wire format and the command line live in part C's `dj_ipc::proto`, so A and C don't depend on each other.)

```rust
// (part C) dj_ipc::proto — the wire format and the command line; shown here for reference
pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 4096;
pub const MAX_LABEL: usize = 48;
pub const MAX_MESSAGE: usize = 120;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Progress { percent: Option<u8>, label: String },
    Done { message: String },
    Notify { message: String },
    Theme { id: Option<String> },
    Snap,
}
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Reply { pub ok: bool, pub text: Option<String>, pub error: Option<String> }
impl Reply { pub fn ok() -> Self; pub fn text(t: impl Into<String>) -> Self; pub fn error(e: impl Into<String>) -> Self; }

/// `{"v":1,"cmd":"progress","percent":40,"label":"deploy"}\n` (serde, internally tagged on "cmd").
pub fn encode_request(r: &Request) -> Vec<u8>;
/// Rejects: over MAX_FRAME, not UTF-8, bad JSON, another version, unknown cmd, percent > 100,
/// an empty notify message. Cleans the text fields (clean_text) on the way in.
pub fn decode_request(frame: &[u8]) -> Result<Request, String>;
pub fn encode_reply(r: &Reply) -> Vec<u8>;
pub fn decode_reply(frame: &[u8]) -> Result<Reply, String>;
/// Trim, collapse whitespace runs to one space, drop control chars, keep at most `max` chars.
pub fn clean_text(s: &str, max: usize) -> String;

/// What `dj <args>` asks for (args without the program name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cli { Send(Request), Help, Version, Scene(String) }
pub fn parse_args(args: &[String]) -> Result<Cli, String>;   // Err = a one-line usage error
pub const USAGE: &str;    // the --help text (commands, examples, exit codes)
```
Parsing: `progress <n|n%> [label words…]` or `progress <label words…>` (first arg not a number → open-ended, all words are the label); `done [message words…]`; `notify <message words…>` (required); `theme [id]`; `snap`; `scene <name>`; `-h/--help/help`; `-V/--version/version`; nothing → Help. Numbers: 0–100 integers, an optional trailing `%`; `101`, `-1`, `4.5` → error.

```rust
// dj.rs — the island's dj activity, time injected
pub const DONE_FOR: Duration = Duration::from_secs(4);
pub const NOTICE_FOR: Duration = Duration::from_secs(5);
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressView { pub label: String, pub percent: Option<u8>, pub done: Option<String> }
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DjView { pub progress: Option<ProgressView>, pub notice: Option<String> }
#[derive(Default)]
pub struct Dj { /* progress + its times, notice + its end */ }
impl Dj {
    pub fn progress(&mut self, percent: Option<u8>, label: String, now: Instant);  // replaces or updates; clears a pending done
    pub fn done(&mut self, message: String, now: Instant);   // shows ✓ message for DONE_FOR, with or without a progress
    pub fn notify(&mut self, message: String, now: Instant); // replaces any notice
    pub fn dismiss_notice(&mut self) -> bool;
    pub fn tick(&mut self, now: Instant) -> bool;   // drops what timed out; true if the view changed
    pub fn next_change(&self, now: Instant) -> Option<Duration>;
    pub fn view(&self) -> DjView;
}
```
Tests (part A, `dj.rs`): update, replace, done with and without progress, done timeout, notice timeout and replace, dismiss, stale expiry, next_change ordering. Tests (part C, `proto`): round trips for every request and reply; every rejection; clean_text (spaces, controls, emoji at the cut, max); every parse case incl. `40%`, open-ended, multi-word labels, bounds, missing args, unknown command.

## Part B — island states (worktree `11-island`)

**Files:** `crates/dj-core/src/island.rs`, `crates/dj-core/src/layout.rs` only (a compile fix elsewhere only if a `match` breaks; report it).

```rust
// layout.rs
pub const DJ_PROGRESS_W: f32 = 300.0; pub const DJ_PROGRESS_H: f32 = 36.0; pub const DJ_PROGRESS_R: f32 = 18.0;
pub const DJ_NOTICE_W: f32 = 360.0;   pub const DJ_NOTICE_H: f32 = 36.0;   pub const DJ_NOTICE_R: f32 = 18.0;
// island.rs
IslandState::DjProgress, IslandState::DjNotice      // two rest states with their own layers (Layers.dj_progress, Layers.dj_notice)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct DjUi { pub progress: bool, pub notice: bool }
impl Island { pub fn set_dj(&mut self, ui: DjUi, tokens: &ResolvedTokens); }
ClickOutcome::DjDismiss          // a click on the notice pill (the app dismisses the notice)
```
Rest-state order: snap thumb > **dj notice** > timer alert > **dj progress** > music compact > timer compact > idle. Like the other facts, a change while a panel is open waits for the collapse. A click on the progress pill expands to Home (as from Idle). Both heights fit `RESERVE_H` (update its test if it lists compact heights). Tests: each priority pair, set_dj from every rest state and under an open panel, sizes, layers, the two clicks.

## Part C — crate `dj-ipc` + the `dj` binary (worktree `11-ipc`)

**Files:** create `crates/dj-ipc/` (`Cargo.toml`, `src/lib.rs`, `src/proto.rs` (the wire format + CLI parsing from the part A section, pure and fully tested), `src/pipe.rs` or similar, `src/bin/dj.rs`); add `"crates/dj-ipc"` to the workspace `members` in the root `Cargo.toml`. Dependencies: `serde`, `serde_json` (workspace), `windows` (workspace) with the features you need (pipes, file I/O, security, threading, remote desktop for the session id); no `dj-core` dependency is needed. Nothing in `app/`.

```rust
pub mod proto;   // the wire format and the command line above (Request, Reply, Cli, parse_args, USAGE, …)
pub use proto::{Reply, Request};
/// `\\.\pipe\daijobu-island-<current user SID string>-<current session id>`.
pub fn pipe_name() -> String;

/// A request taken off the pipe, waiting for the UI thread's answer.
pub struct Incoming { pub request: Request, /* reply channel */ }
impl Incoming { pub fn reply(self, r: Reply); }   // dropping it without a reply answers "busy"

/// The island's end. A thread owns the pipe: create (DACL: current user + SYSTEM, PIPE_REJECT_REMOTE_CLIENTS,
/// FILE_FLAG_FIRST_PIPE_INSTANCE, overlapped), connect, read one frame (2 s limit), decode, hand over through the
/// channel and call `wake`, wait up to 3 s for the reply ("busy" otherwise), write it, disconnect, repeat.
pub struct Server { /* thread, stop event */ }
impl Server {
    pub fn start(wake: Box<dyn Fn() + Send>) -> Result<(Server, std::sync::mpsc::Receiver<Incoming>), String>;
    /// Same with a given name (tests use a unique one).
    pub fn start_named(name: &str, wake: Box<dyn Fn() + Send>) -> Result<(Server, std::sync::mpsc::Receiver<Incoming>), String>;
}
impl Drop for Server { /* signal stop, cancel the pending connect/read, join */ }

#[derive(Debug, PartialEq, Eq)] pub enum SendError { NotRunning, Timeout, Io(String), Protocol(String) }
/// The client: open the pipe (wait ≤ `timeout` if busy), write the request, read the reply.
pub fn send(request: &Request, timeout: Duration) -> Result<Reply, SendError>;
pub fn send_to(name: &str, request: &Request, timeout: Duration) -> Result<Reply, SendError>;
```
`src/bin/dj.rs`: `parse_args` → Help prints `USAGE`; Version prints `dj <crate version>`; Scene prints "dj: scenes are coming soon" (exit 1); Send → `send(.., 3 s)`: ok → print `text` if any, exit 0; island error → `dj: <error>` on stderr, exit 1; NotRunning → "dj: Daijobu Island is not running", exit 2; other errors → exit 1. A usage error prints it plus "see dj --help", exit 1.
Tests: pipe name format (SID string, session); the SDDL string; an **in-process end-to-end** test (a `start_named` server with a unique name, `send_to` a request, answer from the test, check the reply; a silent client is dropped; a reply never given → "busy"; a second server on the same name fails); the binary's exit-code mapping as a pure function.

## Part D — the pills (worktree `11-render`)

**Files:** create `crates/dj-render/src/dj.rs`; `crates/dj-render/src/lib.rs` (mod + `pub use dj::{DjNoticePill, DjProgressPill};`). Not `paint.rs` (the coordinator hooks the layers).

```rust
#[derive(Clone, Copy, Debug)] pub struct DjProgressPill<'a> { pub label: &'a str, pub percent: Option<u8>, pub done: Option<&'a str> }
#[derive(Clone, Copy, Debug)] pub struct DjNoticePill<'a> { pub message: &'a str }
/// Content of the progress pill inside `rect` (device px, the pill's current rect): a lamp, the label (mono 11,
/// "working" when empty, ellipsized), a 4 px bar (track text @ 15 %, fill accent) and the percent right-aligned;
/// open-ended: no fill, "…" instead of the percent; done: a drawn check mark in accent + the message.
pub(crate) fn paint_progress(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, rect: Rect, scale: f32, p: &DjProgressPill);
/// The notice pill: a lamp in `highlight` and the message (font_title 12, ellipsized).
pub(crate) fn paint_notice(canvas: &Canvas, fonts: &mut Fonts, t: &ResolvedTokens, rect: Rect, scale: f32, p: &DjNoticePill);
```
Every glyph drawn must exist in the embedded fonts (no "‥", no "✓" — draw the check as a path). Text never leaves its slot. Tests on CPU raster surfaces: lamp, bar fill proportional to the percent, open-ended without fill, done check pixels in accent, long label cut, notice text inside, nothing outside the pill rect, 150 % scale.

## Part F — wiring (coordinator)

`app`: start the `Server` (`window::wake` as the waker) in `install_shell_integration`, keep the receiver; `Event::Wake` drains `Incoming`s: progress/done/notify → `Dj` + `island.set_dj`; theme (ok / unknown with the list / list); snap → `request_snap`; reply each. `TIMER_DJ` one-shot on `Dj::next_change`; `ClickOutcome::DjDismiss`; `Frame.dj_progress` / `Frame.dj_notice` + the two layers in `paint.rs`; tests through a request handler that doesn't need Win32; README section; AGENTS.md layout line for `dj-ipc`; report; progress.
