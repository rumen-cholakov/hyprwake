//! Quoting helpers for Hyprland's Lua dispatcher syntax (Hyprland >= 0.55).

/// Quote a value as a single-quoted Lua string literal.
pub fn lua_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Quote a value as a Lua long-bracket literal, which needs no escaping at
/// all. Command lines are full of quotes and backslashes, so they travel as
/// long strings; the bracket level is raised until it cannot collide with the
/// payload.
pub fn lua_long_str(s: &str) -> String {
    let mut eq = String::new();
    while s.contains(&format!("]{eq}]")) {
        eq.push('=');
    }
    format!("[{eq}[{s}]{eq}]")
}

/// Quote an argv into a single shell command line.
pub fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(arg: &str) -> String {
    const SAFE: &str = "@%+=:,./-_";
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || SAFE.contains(c))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_string_is_quoted() {
        assert_eq!(lua_str("2 silent"), "'2 silent'");
    }

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(lua_str(r"it's\ok"), r"'it\'s\\ok'");
    }

    #[test]
    fn long_string_needs_no_escaping() {
        assert_eq!(lua_long_str("foo 'bar' \"baz\""), "[[foo 'bar' \"baz\"]]");
    }

    #[test]
    fn long_string_raises_bracket_level_on_collision() {
        assert_eq!(lua_long_str("a]]b"), "[=[a]]b]=]");
    }

    #[test]
    fn long_string_raises_level_repeatedly() {
        assert_eq!(lua_long_str("x]]y]=]z"), "[==[x]]y]=]z]==]");
    }

    #[test]
    fn shell_join_leaves_simple_args_bare() {
        let argv = vec![
            "foot".to_string(),
            "-D".to_string(),
            "/home/user".to_string(),
        ];
        assert_eq!(shell_join(&argv), "foot -D /home/user");
    }

    #[test]
    fn shell_join_quotes_spaces_and_apostrophes() {
        let argv = vec![
            "nvim".to_string(),
            "/home/user/my notes".to_string(),
            "it's".to_string(),
        ];
        assert_eq!(shell_join(&argv), r#"nvim '/home/user/my notes' 'it'\''s'"#);
    }

    #[test]
    fn shell_join_quotes_empty_arg() {
        assert_eq!(shell_join(&["".to_string()]), "''");
    }
}
