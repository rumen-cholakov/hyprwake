//! Recovering a program's session id from its open files.
//!
//! Some programs keep a per-session working directory or state file open
//! whose path contains the session identifier. Claude Code is one: a running
//! session holds `/tmp/claude-<uid>/<project>/<session-id>/tasks`, and
//! `claude --resume <session-id>` reopens that exact conversation.
//!
//! ```text
//! /tmp/claude-1000/-home-rc-Work/b8a0afc7-.../tasks  ->  b8a0afc7-...
//! ```
//!
//! Other programs keep their session index in a database instead, with no
//! per-process trace at all. For those a rule can name a command that prints
//! the session id for a working directory — codex records `cwd` against every
//! thread it has run, so the right one can be looked up.
//!
//! Either way this beats a "continue the most recent one" flag, which
//! collapses every session onto whichever was touched last — restoring three
//! terminals would then reopen the same conversation three times.

/// Match `path` against a pattern and capture the `{id}` it names.
///
/// Patterns are paths whose segments may contain one `*` wildcard, with
/// exactly one segment carrying the `{id}` placeholder:
///
/// ```text
/// /tmp/claude-*/*/{id}/*
/// ```
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

pub fn extract_id(pattern: &str, path: &str) -> Option<String> {
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let path_segments: Vec<&str> = path.split('/').collect();
    if pattern_segments.len() != path_segments.len() {
        return None;
    }

    let mut found = None;
    for (pat, seg) in pattern_segments.iter().zip(path_segments.iter()) {
        if let Some((prefix, suffix)) = pat.split_once("{id}") {
            let id = seg
                .strip_prefix(prefix)
                .and_then(|s| s.strip_suffix(suffix))
                .filter(|s| !s.is_empty())?;
            found = Some(id.to_string());
        } else if !segment_matches(pat, seg) {
            return None;
        }
    }
    found
}

fn segment_matches(pattern: &str, segment: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == segment,
        Some((prefix, suffix)) => {
            segment.len() >= prefix.len() + suffix.len()
                && segment.starts_with(prefix)
                && segment.ends_with(suffix)
        }
    }
}

/// The first id any of `paths` yields for `pattern`.
pub fn find_id<'a>(pattern: &str, paths: impl IntoIterator<Item = &'a str>) -> Option<String> {
    paths.into_iter().find_map(|p| extract_id(pattern, p))
}

/// Substitute a captured id into a program's resume arguments.
pub fn render_args(args: &[String], id: &str) -> Vec<String> {
    args.iter().map(|a| a.replace("{id}", id)).collect()
}

/// Accept an id only if it is a single harmless token.
///
/// The value ends up on a command line, and it comes from a filesystem path
/// or the stdout of a helper command, so it is checked rather than trusted.
pub fn sanitize_id(raw: &str) -> Option<String> {
    let id = raw.lines().next()?.trim();
    if id.is_empty() || id.len() > 128 {
        return None;
    }
    let ok = id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'));
    ok.then(|| id.to_string())
}

/// Fill `{cwd}`, `{cwd_sql}` and `{home}` into an id-command.
///
/// `{cwd_sql}` doubles single quotes so a directory name containing one
/// cannot break out of a SQL string literal.
pub fn render_command(command: &[String], cwd: &str, home: &str) -> Vec<String> {
    command
        .iter()
        .map(|arg| {
            arg.replace("{cwd_sql}", &cwd.replace('\'', "''"))
                .replace("{cwd}", cwd)
                .replace("{home}", home)
        })
        .collect()
}

/// Run an id-command and return what it printed.
///
/// Bounded by a timeout: this runs on every save, including from the
/// watcher, and a helper that blocks must not stall the snapshot.
pub fn run_id_command(command: &[String], timeout: Duration) -> Option<String> {
    let (program, args) = command.split_first()?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // The pipe is read on another thread so a wedged helper can be killed
    // instead of blocking the save.
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    match rx.recv_timeout(timeout) {
        Ok(output) => {
            let _ = child.wait();
            sanitize_id(&output)
        }
        Err(_) => {
            crate::logging::log(format!("resume: id command timed out: {program}"));
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATTERN: &str = "/tmp/claude-*/*/{id}/*";
    const REAL: &str =
        "/tmp/claude-1000/-home-rc-Work/b8a0afc7-1374-4bad-957b-2d5eef6f50a1/tasks";

    #[test]
    fn captures_the_session_id_from_a_real_path() {
        assert_eq!(
            extract_id(PATTERN, REAL).as_deref(),
            Some("b8a0afc7-1374-4bad-957b-2d5eef6f50a1")
        );
    }

    #[test]
    fn a_wildcard_matches_any_single_segment() {
        assert_eq!(
            extract_id("/a/*/{id}", "/a/anything/xyz").as_deref(),
            Some("xyz")
        );
    }

    #[test]
    fn a_wildcard_matches_inside_a_segment() {
        assert!(segment_matches("claude-*", "claude-1000"));
        assert!(segment_matches("*.sock", "hypr.sock"));
        assert!(!segment_matches("claude-*", "codex-1000"));
    }

    #[test]
    fn a_different_depth_never_matches() {
        assert_eq!(extract_id(PATTERN, "/tmp/claude-1000/proj/id"), None);
        assert_eq!(extract_id(PATTERN, "/tmp/claude-1000/proj/id/a/b"), None);
    }

    #[test]
    fn an_unrelated_path_never_matches() {
        assert_eq!(extract_id(PATTERN, "/proc/3986/statm"), None);
        assert_eq!(
            extract_id(PATTERN, "/tmp/other-1000/proj/id/tasks"),
            None
        );
    }

    #[test]
    fn an_empty_id_is_not_a_match() {
        assert_eq!(extract_id("/a/{id}", "/a/"), None);
    }

    #[test]
    fn an_id_with_a_prefix_and_suffix_in_its_segment() {
        assert_eq!(
            extract_id("/run/s-{id}.lock", "/run/s-42.lock").as_deref(),
            Some("42")
        );
    }

    #[test]
    fn find_id_skips_unrelated_open_files() {
        let open = vec!["/proc/3986/statm", "/dev/null", REAL];
        assert_eq!(
            find_id(PATTERN, open).as_deref(),
            Some("b8a0afc7-1374-4bad-957b-2d5eef6f50a1")
        );
    }

    #[test]
    fn find_id_returns_nothing_when_no_file_matches() {
        assert_eq!(find_id(PATTERN, vec!["/dev/null"]), None);
    }

    #[test]
    fn a_plausible_id_is_accepted() {
        assert_eq!(
            sanitize_id("01a02bbb-4956-78a1-bf64-5409e7269c76\n").as_deref(),
            Some("01a02bbb-4956-78a1-bf64-5409e7269c76")
        );
    }

    #[test]
    fn a_dangerous_or_empty_id_is_refused() {
        // The id reaches a command line; anything shell-ish is not an id.
        assert_eq!(sanitize_id("id; rm -rf /"), None);
        assert_eq!(sanitize_id("a b"), None);
        assert_eq!(sanitize_id("$(whoami)"), None);
        assert_eq!(sanitize_id(""), None);
        assert_eq!(sanitize_id("   "), None);
        assert_eq!(sanitize_id(&"x".repeat(200)), None);
    }

    #[test]
    fn only_the_first_line_of_output_is_taken() {
        assert_eq!(sanitize_id("abc\ndef").as_deref(), Some("abc"));
    }

    #[test]
    fn command_placeholders_are_filled() {
        let cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            "q {home} '{cwd_sql}' {cwd}".to_string(),
        ];
        assert_eq!(
            render_command(&cmd, "/home/rc/Work", "/home/rc")[2],
            "q /home/rc '/home/rc/Work' /home/rc/Work"
        );
    }

    #[test]
    fn a_quote_in_a_directory_cannot_escape_a_sql_literal() {
        let cmd = vec!["q '{cwd_sql}'".to_string()];
        assert_eq!(render_command(&cmd, "/home/rc/it's", "/home/rc")[0], "q '/home/rc/it''s'");
    }

    #[test]
    fn an_id_command_returns_its_output() {
        let cmd = vec!["printf".to_string(), "abc-123".to_string()];
        assert_eq!(
            run_id_command(&cmd, Duration::from_secs(5)).as_deref(),
            Some("abc-123")
        );
    }

    #[test]
    fn an_id_command_that_prints_nothing_yields_nothing() {
        let cmd = vec!["true".to_string()];
        assert_eq!(run_id_command(&cmd, Duration::from_secs(5)), None);
    }

    #[test]
    fn a_missing_id_command_is_not_an_error() {
        let cmd = vec!["hyprwake-no-such-helper".to_string()];
        assert_eq!(run_id_command(&cmd, Duration::from_secs(5)), None);
    }

    #[test]
    fn a_wedged_id_command_is_killed() {
        let cmd = vec!["sleep".to_string(), "30".to_string()];
        let start = std::time::Instant::now();
        assert_eq!(run_id_command(&cmd, Duration::from_millis(200)), None);
        assert!(start.elapsed() < Duration::from_secs(5), "must not wait for the child");
    }

    #[test]
    fn arguments_are_rendered_with_the_id() {
        let args = vec!["--resume".to_string(), "{id}".to_string()];
        assert_eq!(render_args(&args, "abc"), vec!["--resume", "abc"]);
    }
}
