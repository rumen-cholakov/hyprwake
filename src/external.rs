//! External providers: capture and relaunch delegated to a helper program.
//!
//! The mechanisms hyprwake knows are built in. `kind = "external"` is the one
//! that is not: it hands a window to a helper you write, which speaks JSON on
//! stdin and stdout. That widens what runs on a save and what reaches a
//! command line, so everything here is about refusing quietly and falling
//! back rather than trusting:
//!
//! - the helper must be a file you own that nobody else can write, in a
//!   directory nobody else can write — otherwise it is not run at all;
//! - it gets the same short timeout as a resume `id_command`;
//! - every argv token it returns is checked before it reaches a command line,
//!   and a workspace it returns must be a workspace selector;
//! - any failure falls back to what hyprwake would have done without it:
//!   replaying the window's own argv.
//!
//! The contract is versioned on its own (`CONTRACT`), separately from the
//! snapshot schema, and documented in `docs/PROVIDERS.md`.

use crate::workspace::WorkspaceRef;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Version of the JSON exchanged with helpers. A helper that answers with
/// another version is not believed.
pub const CONTRACT: u32 = 1;

/// The same budget as a resume `id_command`: capture runs on every save.
pub const TIMEOUT: Duration = Duration::from_secs(3);

/// Largest helper reply read, and largest `state` kept in a snapshot.
const MAX_OUTPUT: u64 = 256 * 1024;
const MAX_STATE: usize = 16 * 1024;
const MAX_ARGS: usize = 256;
const MAX_ARG_LEN: usize = 4096;

/// `[providers.<key>]` with `kind = "external"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalConfig {
    /// Run at every save for each window of a claimed class.
    pub capture: Vec<String>,
    /// Run at restore, before anything is launched. Without it, what capture
    /// returned is launched as it is.
    #[serde(default)]
    pub relaunch: Option<Vec<String>>,
}

/// What capture sends.
#[derive(Debug, Serialize)]
pub struct CaptureRequest<'a> {
    pub contract: u32,
    pub event: &'static str,
    pub class: &'a str,
    pub pid: i32,
    pub cwd: Option<&'a str>,
    pub argv: &'a [String],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureReply {
    contract: u32,
    #[serde(default)]
    argv: Option<Vec<String>>,
    #[serde(default)]
    state: Option<serde_json::Value>,
}

/// What relaunch sends.
#[derive(Debug, Serialize)]
pub struct RelaunchRequest<'a> {
    pub contract: u32,
    pub event: &'static str,
    pub class: &'a str,
    pub workspace: String,
    pub argv: &'a [String],
    pub state: Option<&'a serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelaunchReply {
    contract: u32,
    argv: Vec<String>,
    #[serde(default)]
    workspace: Option<String>,
}

/// A capture the helper vouched for.
#[derive(Debug, Clone, PartialEq)]
pub struct Captured {
    /// What to launch, when the helper said; otherwise the window's own.
    pub argv: Option<Vec<String>>,
    pub state: Option<serde_json::Value>,
}

/// A relaunch the helper vouched for.
#[derive(Debug, Clone, PartialEq)]
pub struct Relaunch {
    pub argv: Vec<String>,
    pub workspace: Option<WorkspaceRef>,
}

/// Why a helper's answer was not used. Logged, and shown by `--dry-run`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum HelperError {
    #[error("no helper command given")]
    Empty,
    #[error("helper {0} is not an absolute path to a file")]
    NotAFile(String),
    #[error("helper {path} is not trusted: {why}")]
    Untrusted { path: String, why: String },
    #[error("helper {0} could not be started: {1}")]
    Spawn(String, String),
    #[error("helper {0} did not answer within {1:?}")]
    Timeout(String, Duration),
    #[error("helper {0} exited unsuccessfully")]
    Failed(String),
    #[error("helper reply is not valid JSON for this contract: {0}")]
    Reply(String),
    #[error("helper speaks contract {found}, this hyprwake speaks {CONTRACT}")]
    Contract { found: u32 },
    #[error("helper returned an argv that cannot be launched: {0}")]
    Argv(String),
    #[error("helper returned a workspace that is not a workspace selector: {0:?}")]
    Workspace(String),
    #[error("helper state is larger than {MAX_STATE} bytes")]
    StateTooLarge,
}

/// Ask the capture helper about one window.
pub fn capture(config: &ExternalConfig, request: &CaptureRequest) -> Result<Captured, HelperError> {
    let input = serde_json::to_string(request).map_err(|e| HelperError::Reply(e.to_string()))?;
    let output = run(&config.capture, &input, TIMEOUT)?;
    parse_capture(&output)
}

/// Ask the relaunch helper what to start for one saved window, and the raw
/// reply, which `--dry-run` shows as it came.
pub fn relaunch(
    command: &[String],
    request: &RelaunchRequest,
) -> (Result<Relaunch, HelperError>, Option<String>) {
    let input = match serde_json::to_string(request) {
        Ok(input) => input,
        Err(e) => return (Err(HelperError::Reply(e.to_string())), None),
    };
    match run(command, &input, TIMEOUT) {
        Ok(output) => (parse_relaunch(&output), Some(output)),
        Err(e) => (Err(e), None),
    }
}

fn parse_capture(output: &str) -> Result<Captured, HelperError> {
    let reply: CaptureReply =
        serde_json::from_str(output.trim()).map_err(|e| HelperError::Reply(e.to_string()))?;
    if reply.contract != CONTRACT {
        return Err(HelperError::Contract {
            found: reply.contract,
        });
    }
    if let Some(argv) = &reply.argv {
        check_argv(argv)?;
    }
    if let Some(state) = &reply.state {
        if serde_json::to_string(state).map_or(true, |s| s.len() > MAX_STATE) {
            return Err(HelperError::StateTooLarge);
        }
    }
    Ok(Captured {
        argv: reply.argv,
        state: reply.state,
    })
}

fn parse_relaunch(output: &str) -> Result<Relaunch, HelperError> {
    let reply: RelaunchReply =
        serde_json::from_str(output.trim()).map_err(|e| HelperError::Reply(e.to_string()))?;
    if reply.contract != CONTRACT {
        return Err(HelperError::Contract {
            found: reply.contract,
        });
    }
    check_argv(&reply.argv)?;
    let workspace = match reply.workspace {
        None => None,
        Some(s) => Some(WorkspaceRef::from_selector(&s).ok_or(HelperError::Workspace(s))?),
    };
    Ok(Relaunch {
        argv: reply.argv,
        workspace,
    })
}

/// An argv that can go on a command line: something to run, no flag where
/// the program belongs, nothing that could end or split a line.
///
/// Shell metacharacters are fine: every argument is quoted on the way to the
/// compositor, so `$(…)` arrives as a literal argument, which is all a
/// program ever receives from a well-formed argv.
pub fn check_argv(argv: &[String]) -> Result<(), HelperError> {
    let program = argv
        .first()
        .ok_or_else(|| HelperError::Argv("empty".into()))?;
    if program.is_empty() || program.starts_with('-') {
        return Err(HelperError::Argv(format!(
            "{program:?} is not a program to run"
        )));
    }
    if argv.len() > MAX_ARGS {
        return Err(HelperError::Argv(format!("more than {MAX_ARGS} arguments")));
    }
    for arg in argv {
        if arg.len() > MAX_ARG_LEN {
            return Err(HelperError::Argv(format!(
                "an argument is longer than {MAX_ARG_LEN} bytes"
            )));
        }
        if arg.chars().any(char::is_control) {
            return Err(HelperError::Argv(
                "an argument contains a control character".into(),
            ));
        }
    }
    Ok(())
}

/// `~/` expanded, so a config can name a helper under the home directory.
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// Whether hyprwake will run `program` as a helper: an absolute path to a
/// file owned by you, which nobody else can write, in a directory nobody
/// else can write. Symbolic links are followed, and the checks apply to what
/// they point at.
pub fn trusted(program: &str) -> Result<PathBuf, HelperError> {
    let path = expand_home(program);
    if !path.is_absolute() {
        return Err(HelperError::NotAFile(program.to_string()));
    }
    let resolved =
        std::fs::canonicalize(&path).map_err(|_| HelperError::NotAFile(program.to_string()))?;
    let meta =
        std::fs::metadata(&resolved).map_err(|_| HelperError::NotAFile(program.to_string()))?;
    if !meta.is_file() {
        return Err(HelperError::NotAFile(program.to_string()));
    }
    let untrusted = |why: &str| HelperError::Untrusted {
        path: resolved.display().to_string(),
        why: why.to_string(),
    };
    let me = current_uid().ok_or_else(|| untrusted("cannot tell who is running hyprwake"))?;
    if meta.uid() != me {
        return Err(untrusted("owned by someone else"));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(untrusted("writable by group or others"));
    }
    if meta.mode() & 0o100 == 0 {
        return Err(untrusted("not executable"));
    }
    let dir = resolved.parent().unwrap_or(Path::new("/"));
    let dir_meta = std::fs::metadata(dir).map_err(|_| untrusted("its directory is unreadable"))?;
    if dir_meta.uid() != me && dir_meta.uid() != 0 {
        return Err(untrusted("its directory is owned by someone else"));
    }
    if dir_meta.mode() & 0o022 != 0 {
        return Err(untrusted("its directory is writable by group or others"));
    }
    Ok(resolved)
}

/// The effective uid, from the owner of this process's own `/proc` entry.
fn current_uid() -> Option<u32> {
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

/// Run a trusted helper with `input` on stdin, returning its stdout.
fn run(command: &[String], input: &str, timeout: Duration) -> Result<String, HelperError> {
    let (program, args) = command.split_first().ok_or(HelperError::Empty)?;
    let path = trusted(program)?;
    let shown = path.display().to_string();
    let mut child = Command::new(&path)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| HelperError::Spawn(shown.clone(), e.to_string()))?;

    // Both pipes on their own threads: a helper that writes before it reads,
    // or never reads at all, must not wedge the save.
    let mut stdin = child.stdin.take();
    let payload = input.as_bytes().to_vec();
    std::thread::spawn(move || {
        if let Some(stdin) = stdin.as_mut() {
            let _ = stdin.write_all(&payload);
        }
    });
    let stdout = child.stdout.take();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(stdout) = stdout {
            let _ = stdout.take(MAX_OUTPUT).read_to_string(&mut buf);
        }
        let _ = tx.send(buf);
    });

    match rx.recv_timeout(timeout) {
        Ok(output) => match child.wait() {
            Ok(status) if status.success() => Ok(output),
            _ => Err(HelperError::Failed(shown)),
        },
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(HelperError::Timeout(shown, timeout))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A helper script in a private temporary directory.
    fn helper(dir: &Path, name: &str, body: &str, mode: u32) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path.display().to_string()
    }

    fn private_dir() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    fn request<'a>(argv: &'a [String]) -> CaptureRequest<'a> {
        CaptureRequest {
            contract: CONTRACT,
            event: "capture",
            class: "myapp",
            pid: 42,
            cwd: Some("/w"),
            argv,
        }
    }

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_capture_round_trips_argv_and_state() {
        let dir = private_dir();
        // Echo the request's class and pid back as state, to prove the
        // helper saw them.
        let h = helper(
            dir.path(),
            "cap",
            r#"read req; pid=$(printf %s "$req" | sed 's/.*"pid":\([0-9]*\).*/\1/'); printf '{"contract":1,"argv":["myapp","--open","x y"],"state":{"pid":%s}}' "$pid""#,
            0o700,
        );
        let argv = strings(&["myapp"]);
        let got = capture(
            &ExternalConfig {
                capture: vec![h],
                relaunch: None,
            },
            &request(&argv),
        )
        .unwrap();
        assert_eq!(got.argv, Some(strings(&["myapp", "--open", "x y"])));
        assert_eq!(got.state, Some(serde_json::json!({"pid": 42})));
    }

    #[test]
    fn a_helper_anyone_else_can_write_is_never_run() {
        let dir = private_dir();
        let marker = dir.path().join("ran");
        let h = helper(
            dir.path(),
            "cap",
            &format!("touch {}; echo '{{\"contract\":1}}'", marker.display()),
            0o722,
        );
        let err = capture(
            &ExternalConfig {
                capture: vec![h],
                relaunch: None,
            },
            &request(&[]),
        )
        .unwrap_err();
        assert!(matches!(err, HelperError::Untrusted { .. }), "{err}");
        assert!(!marker.exists(), "an untrusted helper must not run at all");
    }

    #[test]
    fn a_helper_in_a_shared_directory_is_not_trusted() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        let h = helper(dir.path(), "cap", "echo '{\"contract\":1}'", 0o700);
        let err = trusted(&h).unwrap_err();
        assert!(err.to_string().contains("directory"), "{err}");
    }

    #[test]
    fn a_relative_or_missing_helper_is_refused() {
        assert!(matches!(trusted("cap"), Err(HelperError::NotAFile(_))));
        assert!(matches!(
            trusted("/nonexistent/helper"),
            Err(HelperError::NotAFile(_))
        ));
    }

    #[test]
    fn a_wedged_helper_is_killed() {
        let dir = private_dir();
        let h = helper(dir.path(), "cap", "sleep 30", 0o700);
        let started = std::time::Instant::now();
        let err = run(&[h], "{}", Duration::from_millis(200)).unwrap_err();
        assert!(matches!(err, HelperError::Timeout(..)), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_failing_helper_is_a_failure_whatever_it_printed() {
        let dir = private_dir();
        let h = helper(dir.path(), "cap", "echo '{\"contract\":1}'; exit 3", 0o700);
        assert!(matches!(
            run(&[h], "{}", TIMEOUT),
            Err(HelperError::Failed(_))
        ));
    }

    #[test]
    fn replies_are_checked_before_anything_is_believed() {
        for (reply, want) in [
            ("not json", "JSON"),
            (r#"{"contract":2}"#, "contract 2"),
            (r#"{"contract":1,"argv":[]}"#, "empty"),
            (r#"{"contract":1,"argv":["--flag"]}"#, "not a program"),
            (r#"{"contract":1,"argv":["a","b\nc"]}"#, "control"),
            (r#"{"contract":1,"surprise":true}"#, "surprise"),
        ] {
            let err = parse_capture(reply).unwrap_err().to_string();
            assert!(err.contains(want), "{reply}: {err}");
        }
        let big = format!(
            r#"{{"contract":1,"state":"{}"}}"#,
            "x".repeat(MAX_STATE + 1)
        );
        assert_eq!(parse_capture(&big), Err(HelperError::StateTooLarge));
        // Shell syntax in an argument is just an argument.
        assert!(parse_capture(r#"{"contract":1,"argv":["a","$(rm -rf ~)"]}"#).is_ok());
    }

    #[test]
    fn a_relaunch_workspace_must_be_a_selector() {
        let ok = parse_relaunch(r#"{"contract":1,"argv":["a"],"workspace":"name:mail"}"#).unwrap();
        assert_eq!(ok.workspace.unwrap().selector(), "name:mail");
        assert!(matches!(
            parse_relaunch(r#"{"contract":1,"argv":["a"],"workspace":"3 silent; exec x"}"#),
            Err(HelperError::Workspace(_))
        ));
        assert!(parse_relaunch(r#"{"contract":1,"argv":["a"]}"#)
            .unwrap()
            .workspace
            .is_none());
    }

    #[test]
    fn the_relaunch_reply_is_returned_verbatim_for_the_dry_run() {
        let dir = private_dir();
        let h = helper(
            dir.path(),
            "rel",
            r#"cat >/dev/null; echo '{"contract":1,"argv":["myapp","--resume","7"]}'"#,
            0o700,
        );
        let argv = strings(&["myapp"]);
        let (result, raw) = relaunch(
            &[h],
            &RelaunchRequest {
                contract: CONTRACT,
                event: "relaunch",
                class: "myapp",
                workspace: "3".into(),
                argv: &argv,
                state: None,
            },
        );
        assert_eq!(result.unwrap().argv, strings(&["myapp", "--resume", "7"]));
        assert!(raw.unwrap().contains("\"--resume\""));
    }
}
