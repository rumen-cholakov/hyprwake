//! Recovering a program's session id from its open files.
//!
//! Some programs keep a per-session working directory or state file open
//! whose path contains the session identifier. Claude Code is one: a running
//! session holds `/tmp/claude-<uid>/<project>/<session-id>/tasks`, and
//! `claude --resume <session-id>` reopens that exact conversation.
//!
//! This is worth the trouble over a "continue the most recent one" flag,
//! which collapses every session in a directory onto whichever was touched
//! last — restoring three terminals would then reopen the same conversation
//! three times.

/// Match `path` against a pattern and capture the `{id}` it names.
///
/// Patterns are paths whose segments may contain one `*` wildcard, with
/// exactly one segment carrying the `{id}` placeholder:
///
/// ```text
/// /tmp/claude-*/*/{id}/*
/// ```
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
    fn arguments_are_rendered_with_the_id() {
        let args = vec!["--resume".to_string(), "{id}".to_string()];
        assert_eq!(render_args(&args, "abc"), vec!["--resume", "abc"]);
    }
}
