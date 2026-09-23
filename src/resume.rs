//! Recovering a program's session id from its open files.
//!
//! Some programs keep a per-session working directory or state file open
//! whose path contains the session identifier. Claude Code is one: a running
//! session holds `/tmp/claude-<uid>/<project>/<session-id>/tasks`, and
//! `claude --resume <session-id>` reopens that exact conversation.
//!
//! ```text
//! /tmp/claude-1000/-home-user-Work/0f1e2d3c-.../tasks  ->  0f1e2d3c-...
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
///
/// Returns `None` — the rule then falls back — when a value would be spliced
/// into *script text*: the argument after `sh -c`, `python -c`, `perl -e` and
/// the like. A working directory is attacker-controlled (a subdirectory of a
/// cloned repository can be named anything) and this runs on every save, so
/// a name such as `$(…)` placed inside a script would be executed. Values in
/// ordinary arguments are passed through untouched, because no interpreter
/// ever parses them; the safe way to hand a value to a script is as a
/// positional parameter (`sh -c '… "$1" …' sh {cwd}`), which is how the
/// shipped rules do it.
pub fn render_command(command: &[String], cwd: &str, home: &str) -> Option<Vec<String>> {
    let script = script_argument(command);
    let mut out = Vec::with_capacity(command.len());
    for (i, arg) in command.iter().enumerate() {
        if Some(i) == script {
            let uses_cwd = arg.contains("{cwd}") || arg.contains("{cwd_sql}");
            let uses_home = arg.contains("{home}");
            if (uses_cwd && !inert_in_script(cwd)) || (uses_home && !inert_in_script(home)) {
                crate::logging::log(
                    "resume: refusing to splice a directory name containing shell-active \
                     characters into script text; pass it as a positional parameter instead",
                );
                return None;
            }
        }
        out.push(
            arg.replace("{cwd_sql}", &cwd.replace('\'', "''"))
                .replace("{cwd}", cwd)
                .replace("{home}", home),
        );
    }
    Some(out)
}

/// How an interpreter is told that an argument is program text.
enum ScriptFlag {
    /// `-c`, or a short-flag cluster ending in it: `sh -ec '…'`. Shells take
    /// `-e` as errexit, so `e` must not count here.
    ClusterEndingIn(char),
    /// Exactly one of these spellings.
    Exact(&'static [&'static str]),
}

/// Interpreters whose script argument is code rather than data.
fn script_flag(program: &str) -> Option<ScriptFlag> {
    // `python3.12` and `python3` are `python`.
    let base = program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    Some(match base {
        "sh" | "bash" | "dash" | "zsh" | "ksh" | "mksh" | "ash" | "busybox" => {
            ScriptFlag::ClusterEndingIn('c')
        }
        "fish" => ScriptFlag::Exact(&["-c", "--command"]),
        "python" => ScriptFlag::ClusterEndingIn('c'),
        "perl" | "ruby" => ScriptFlag::ClusterEndingIn('e'),
        "node" | "bun" => ScriptFlag::Exact(&["-e", "--eval", "-p", "--print"]),
        "lua" => ScriptFlag::Exact(&["-e"]),
        _ => return None,
    })
}

/// The index of the argument an interpreter will parse as code, if any.
///
/// Looks past a leading `env` and its flags and `NAME=value` assignments,
/// since `env sh -c '…'` is as much a script as `sh -c '…'`.
pub(crate) fn script_argument(command: &[String]) -> Option<usize> {
    let basename = |a: &str| -> String {
        std::path::Path::new(a)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(a)
            .to_string()
    };
    let mut i = 0;
    if command.first().is_some_and(|a| basename(a) == "env") {
        i = 1;
        while command
            .get(i)
            .is_some_and(|a| a.starts_with('-') || a.contains('='))
        {
            i += 1;
        }
    }
    let flag = script_flag(&basename(command.get(i)?))?;
    command[i + 1..]
        .iter()
        .position(|a| match &flag {
            ScriptFlag::ClusterEndingIn(c) => {
                a.len() >= 2 && a.starts_with('-') && !a.starts_with("--") && a.ends_with(*c)
            }
            ScriptFlag::Exact(spellings) => spellings.contains(&a.as_str()),
        })
        .map(|p| i + 1 + p + 1)
}

/// Whether a value can sit anywhere in script text without the interpreter
/// acting on it: no expansion, no quote that could close a literal, no
/// command separator, no escape.
fn inert_in_script(value: &str) -> bool {
    !value.chars().any(|c| {
        c.is_control()
            || matches!(
                c,
                '$' | '`' | '\\' | '"' | '\'' | ';' | '&' | '|' | '<' | '>' | '(' | ')' | '{' | '}'
            )
    })
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
        "/tmp/claude-1000/-home-user-Work/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0/tasks";

    #[test]
    fn captures_the_session_id_from_a_real_path() {
        assert_eq!(
            extract_id(PATTERN, REAL).as_deref(),
            Some("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0")
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
        assert_eq!(extract_id(PATTERN, "/tmp/other-1000/proj/id/tasks"), None);
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
            Some("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0")
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
            render_command(&cmd, "/home/user/Work", "/home/user").unwrap()[2],
            "q /home/user '/home/user/Work' /home/user/Work"
        );
    }

    #[test]
    fn a_quote_in_a_directory_cannot_escape_a_sql_literal() {
        let cmd = vec!["q '{cwd_sql}'".to_string()];
        assert_eq!(
            render_command(&cmd, "/home/user/it's", "/home/user").unwrap()[0],
            "q '/home/user/it''s'"
        );
    }

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_script_argument_is_found_per_interpreter() {
        assert_eq!(script_argument(&args(&["sh", "-c", "x"])), Some(2));
        assert_eq!(script_argument(&args(&["/bin/bash", "-ec", "x"])), Some(2));
        // For a shell -e is errexit, not a script: the script is after -c.
        assert_eq!(script_argument(&args(&["sh", "-e", "-c", "x"])), Some(3));
        assert_eq!(
            script_argument(&args(&["env", "A=1", "sh", "-c", "x"])),
            Some(4)
        );
        assert_eq!(
            script_argument(&args(&["/usr/bin/env", "-i", "bash", "-c", "x"])),
            Some(4)
        );
        assert_eq!(script_argument(&args(&["python3.12", "-c", "x"])), Some(2));
        assert_eq!(script_argument(&args(&["perl", "-ne", "x"])), Some(2));
        assert_eq!(script_argument(&args(&["node", "--eval", "x"])), Some(2));
        assert_eq!(script_argument(&args(&["fish", "--command", "x"])), Some(2));
    }

    #[test]
    fn a_program_that_is_not_an_interpreter_has_no_script() {
        assert_eq!(
            script_argument(&args(&["sqlite3", "db", "SELECT '{cwd_sql}'"])),
            None
        );
        assert_eq!(script_argument(&args(&["sh", "script.sh", "{cwd}"])), None);
        assert_eq!(script_argument(&args(&[])), None);
    }

    /// The pattern 0.1.3 shipped for codex, and documented in the README for
    /// people to copy: `{cwd_sql}` inside a double-quoted string inside the
    /// script. Configs carrying it still exist; the guard is what protects
    /// them.
    fn legacy_codex() -> Vec<String> {
        args(&[
            "sh",
            "-c",
            "db=$(ls -t {home}/.codex/state_*.sqlite | head -1); \
             sqlite3 \"file:$db?mode=ro\" \"SELECT id FROM threads WHERE cwd = '{cwd_sql}';\"",
        ])
    }

    #[test]
    fn a_hostile_name_is_refused_in_script_text() {
        for cwd in [
            "/w/$(id)",
            "/w/`id`",
            "/w/a\"; id; \"",
            "/w/a;id",
            "/w/a\nid",
            "/w/${HOME}",
        ] {
            assert_eq!(
                render_command(&legacy_codex(), cwd, "/home/u"),
                None,
                "{cwd:?}"
            );
        }
    }

    #[test]
    fn an_ordinary_name_still_renders_in_script_text() {
        let rendered = render_command(&legacy_codex(), "/home/u/Проекти/api-v2.1", "/home/u")
            .expect("a plain directory must not be refused");
        assert!(rendered[2].contains("cwd = '/home/u/Проекти/api-v2.1'"));
    }

    #[test]
    fn any_name_is_passed_through_as_an_ordinary_argument() {
        // Outside script text nothing parses the value, so nothing is refused.
        let cmd = args(&["sqlite3", "db", "SELECT id WHERE d = '{cwd_sql}'"]);
        let rendered = render_command(&cmd, "/w/it's $(id)", "/home/u").unwrap();
        assert_eq!(rendered[2], "SELECT id WHERE d = '/w/it''s $(id)'");
    }

    #[test]
    fn a_placeholder_passed_positionally_is_never_refused() {
        let cmd = args(&["sh", "-c", "printf %s \"$1\"", "sh", "{cwd}"]);
        let rendered = render_command(&cmd, "/w/$(id)", "/home/u").unwrap();
        assert_eq!(rendered[4], "/w/$(id)");
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
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must not wait for the child"
        );
    }

    #[test]
    fn arguments_are_rendered_with_the_id() {
        let args = vec!["--resume".to_string(), "{id}".to_string()];
        assert_eq!(render_args(&args, "abc"), vec!["--resume", "abc"]);
    }
}
