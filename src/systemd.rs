//! `systemctl --user`: the only way hyprwake talks to systemd.
//!
//! Under `cfg(test)` nothing reaches systemd — calls are recorded instead, so
//! a test can assert what *would* have been run. The unit names hyprwake uses
//! are real ones: a test that installed a unit into a temporary directory and
//! then uninstalled it ran `disable --now hyprwake-watch.service` against the
//! developer's own systemd, and so stopped and disabled their live watcher on
//! every `cargo test`. A temporary unit *file* does not make the unit *name*
//! temporary.

/// Run `systemctl --user <args>`, reporting whether it succeeded. Output is
/// discarded; callers that care about state ask with `is-active --quiet`.
pub fn user(args: &[&str]) -> bool {
    #[cfg(test)]
    {
        CALLS.with(|calls| {
            calls
                .borrow_mut()
                .push(args.iter().map(|a| a.to_string()).collect())
        });
        false
    }
    #[cfg(not(test))]
    {
        std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
thread_local! {
    static CALLS: std::cell::RefCell<Vec<Vec<String>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Every `systemctl --user` call this test thread would have made, oldest
/// first, clearing the record.
#[cfg(test)]
pub fn take_calls() -> Vec<Vec<String>> {
    CALLS.with(|calls| calls.take())
}

#[cfg(test)]
mod tests {
    /// No other module may reach systemd: this is what keeps a test from
    /// touching the user's units again.
    #[test]
    fn nothing_else_runs_systemctl() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&src).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == "systemd.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if text.contains(&format!("Command::new({:?})", "systemctl")) {
                offenders.push(path.display().to_string());
            }
        }
        assert!(
            offenders.is_empty(),
            "systemctl run outside systemd.rs: {offenders:?}"
        );
    }

    #[test]
    fn calls_are_recorded_not_run() {
        let _ = super::take_calls();
        assert!(!super::user(&[
            "is-active",
            "--quiet",
            "hyprwake-watch.service"
        ]));
        assert_eq!(
            super::take_calls(),
            vec![vec!["is-active", "--quiet", "hyprwake-watch.service"]]
        );
    }
}
