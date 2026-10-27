//! `dj`: show activity on Daijobu Island from a terminal or a script.
//!
//! Exit codes: 0 ok, 1 usage error or an error from the island, 2 the island is not
//! running. Text for the caller goes to stdout, every problem to stderr.

use dj_ipc::proto::{parse_args, Cli, USAGE};
use dj_ipc::{Reply, Request, SendError};
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

/// How long `dj` waits for the pipe and the island's answer together.
const SEND_TIMEOUT: Duration = Duration::from_secs(3);

/// What a run prints and returns.
#[derive(Debug, PartialEq, Eq)]
struct Output {
    stdout: String,
    stderr: String,
    code: u8,
}

impl Output {
    fn ok(stdout: impl Into<String>) -> Self {
        Output {
            stdout: stdout.into(),
            stderr: String::new(),
            code: 0,
        }
    }

    fn fail(code: u8, stderr: impl Into<String>) -> Self {
        Output {
            stdout: String::new(),
            stderr: stderr.into(),
            code,
        }
    }
}

/// The island's reply as output: text on stdout, or its error on stderr with exit code 1.
fn reply_output(reply: Reply) -> Output {
    if reply.ok {
        Output::ok(reply.text.unwrap_or_default())
    } else {
        let why = reply
            .error
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| "the island refused the request".to_string());
        Output::fail(1, format!("dj: {why}"))
    }
}

/// A failure to reach the island as output: 2 if it is not running, 1 otherwise.
fn send_error_output(error: SendError) -> Output {
    match error {
        SendError::NotRunning => Output::fail(2, "dj: Daijobu Island is not running"),
        other => Output::fail(1, format!("dj: {other}")),
    }
}

/// Everything `dj` does, with the pipe behind `send` so it can be tested without one.
fn run(args: &[String], send: impl FnOnce(&Request) -> Result<Reply, SendError>) -> Output {
    match parse_args(args) {
        Err(usage) => Output::fail(1, format!("dj: {usage}\nsee dj --help")),
        Ok(Cli::Help) => Output::ok(USAGE),
        Ok(Cli::Version) => Output::ok(format!("dj {}", env!("CARGO_PKG_VERSION"))),
        Ok(Cli::Scene(_)) => Output::fail(1, "dj: scenes are coming soon"),
        Ok(Cli::Send(request)) => match send(&request) {
            Ok(reply) => reply_output(reply),
            Err(error) => send_error_output(error),
        },
    }
}

/// Writes `text` and a final line break unless it already ends with one.
fn emit(mut out: impl Write, text: &str) {
    if text.is_empty() {
        return;
    }
    // A closed pipe (`dj theme | head`) is not worth a panic or a message.
    let _ = out.write_all(text.as_bytes());
    if !text.ends_with('\n') {
        let _ = out.write_all(b"\n");
    }
    let _ = out.flush();
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let output = run(&args, |request| dj_ipc::send(request, SEND_TIMEOUT));
    emit(std::io::stdout().lock(), &output.stdout);
    emit(std::io::stderr().lock(), &output.stderr);
    ExitCode::from(output.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// Runs with a fake island that answers `reply` and records the request it got.
    fn run_with(a: &[&str], reply: Result<Reply, SendError>) -> (Output, Option<Request>) {
        let seen = RefCell::new(None);
        let out = run(&args(a), |request| {
            *seen.borrow_mut() = Some(request.clone());
            reply
        });
        (out, seen.into_inner())
    }

    /// Runs a command that must not reach the island.
    fn run_offline(a: &[&str]) -> Output {
        run(&args(a), |_| {
            panic!("this command must not contact the island")
        })
    }

    #[test]
    fn help_prints_the_usage() {
        let cases: [&[&str]; 4] = [&[], &["--help"], &["-h"], &["help"]];
        for a in cases {
            let out = run_offline(a);
            assert_eq!(out, Output::ok(USAGE), "{a:?}");
            assert_eq!(out.code, 0);
        }
    }

    #[test]
    fn version_prints_the_crate_version() {
        for a in [&["--version"][..], &["-V"], &["version"]] {
            let out = run_offline(a);
            assert_eq!(out.stdout, format!("dj {}", env!("CARGO_PKG_VERSION")));
            assert!(out.stderr.is_empty());
            assert_eq!(out.code, 0);
        }
        assert!(env!("CARGO_PKG_VERSION").split('.').count() == 3);
    }

    #[test]
    fn scenes_are_coming_soon() {
        let out = run_offline(&["scene", "focus"]);
        assert_eq!(out, Output::fail(1, "dj: scenes are coming soon"));
    }

    #[test]
    fn usage_errors_exit_1_with_a_hint() {
        for a in [
            &["frobnicate"][..],
            &["notify"],
            &["progress", "101"],
            &["progress", "-1"],
            &["progress", "4.5"],
            &["snap", "now"],
            &["theme", "a", "b"],
            &["scene"],
        ] {
            let out = run_offline(a);
            assert_eq!(out.code, 1, "{a:?}");
            assert!(out.stdout.is_empty());
            assert!(out.stderr.starts_with("dj: "), "{a:?}: {}", out.stderr);
            assert!(
                out.stderr.ends_with("see dj --help"),
                "{a:?}: {}",
                out.stderr
            );
        }
    }

    #[test]
    fn a_command_is_sent_as_parsed() {
        let (out, seen) = run_with(&["progress", "40%", "deploy"], Ok(Reply::ok()));
        assert_eq!(out, Output::ok(""));
        assert_eq!(
            seen,
            Some(Request::Progress {
                percent: Some(40),
                label: "deploy".into()
            })
        );
        let (_, seen) = run_with(&["done"], Ok(Reply::ok()));
        assert_eq!(
            seen,
            Some(Request::Done {
                message: "done".into()
            })
        );
        let (_, seen) = run_with(&["notify", "tests", "failed"], Ok(Reply::ok()));
        assert_eq!(
            seen,
            Some(Request::Notify {
                message: "tests failed".into()
            })
        );
        let (_, seen) = run_with(&["theme"], Ok(Reply::ok()));
        assert_eq!(seen, Some(Request::Theme { id: None }));
        let (_, seen) = run_with(&["snap"], Ok(Reply::ok()));
        assert_eq!(seen, Some(Request::Snap));
    }

    #[test]
    fn ok_reply_prints_its_text_and_exits_0() {
        let (out, _) = run_with(&["snap"], Ok(Reply::ok()));
        assert_eq!(out.code, 0);
        assert!(out.stdout.is_empty() && out.stderr.is_empty());

        let (out, _) = run_with(&["theme"], Ok(Reply::text("* crt\n  paper-zine")));
        assert_eq!(out, Output::ok("* crt\n  paper-zine"));
    }

    #[test]
    fn island_errors_exit_1_on_stderr() {
        let (out, _) = run_with(&["theme", "nope"], Ok(Reply::error("unknown theme 'nope'")));
        assert_eq!(out, Output::fail(1, "dj: unknown theme 'nope'"));
        let (out, _) = run_with(&["snap"], Ok(Reply::error("busy")));
        assert_eq!(out, Output::fail(1, "dj: busy"));
        // an error reply without a reason still fails
        let (out, _) = run_with(&["snap"], Ok(Reply::default()));
        assert_eq!(out.code, 1);
        assert!(out.stderr.starts_with("dj: "));
        assert!(out.stdout.is_empty());
    }

    #[test]
    fn not_running_exits_2() {
        let (out, _) = run_with(&["progress", "1"], Err(SendError::NotRunning));
        assert_eq!(out, Output::fail(2, "dj: Daijobu Island is not running"));
    }

    #[test]
    fn other_send_errors_exit_1() {
        for error in [
            SendError::Timeout,
            SendError::Io("access denied".into()),
            SendError::Protocol("bad JSON".into()),
        ] {
            let (out, _) = run_with(&["snap"], Err(error.clone()));
            assert_eq!(out.code, 1, "{error:?}");
            assert!(out.stdout.is_empty());
            assert!(out.stderr.starts_with("dj: "), "{}", out.stderr);
            assert!(!out.stderr.contains("not running"), "{}", out.stderr);
        }
    }

    #[test]
    fn emit_adds_one_line_break() {
        let mut buf = Vec::new();
        emit(&mut buf, "a");
        emit(&mut buf, "b\n");
        emit(&mut buf, "");
        assert_eq!(buf, b"a\nb\n");
    }
}
