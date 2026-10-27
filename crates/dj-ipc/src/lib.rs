//! Daijobu Island's local API: a per-user named pipe, the client behind the `dj` command
//! and the wire format they share.
//!
//! - [`proto`] is pure: the JSON frames, text cleaning and the command line parser.
//! - The pipe side is Win32: [`Server`] is the island's end (a thread that owns the pipe
//!   and hands each request to the UI thread through a channel), [`send`] / [`send_to`]
//!   are the client's end.
//!
//! The pipe is named `\\.\pipe\daijobu-island-<user SID>-<session id>` ([`pipe_name`]), so
//! every user and every logon session has its own. Only that user and SYSTEM can open it,
//! remote clients are refused, and the island creates it with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE` so nobody can squat the name before it starts.

mod pipe;
pub mod proto;

pub use pipe::{pipe_name, send, send_to, Incoming, SendError, Server};
pub use proto::{Reply, Request};
