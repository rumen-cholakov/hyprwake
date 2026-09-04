use assert_cmd::Command;
use predicates::str::contains;

fn hyprwake() -> Command {
    // Cargo points this at the freshly built binary for integration tests.
    Command::new(env!("CARGO_BIN_EXE_hyprwake"))
}

#[test]
fn help_lists_the_main_commands() {
    hyprwake()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("save"))
        .stdout(contains("restore"))
        .stdout(contains("watch"))
        .stdout(contains("status"))
        .stdout(contains("doctor"));
}

#[test]
fn version_is_reported() {
    hyprwake()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn config_reports_paths_and_detected_terminals() {
    hyprwake()
        .arg("config")
        .assert()
        .success()
        .stdout(contains("config.toml"))
        .stdout(contains("Sessions:"))
        // Regression: with no config file the terminals table used to come
        // back empty, silently disabling terminal capture.
        .stdout(contains("foot"));
}

#[test]
fn an_unknown_command_fails() {
    hyprwake()
        .arg("definitely-not-a-command")
        .assert()
        .failure();
}

#[test]
fn restoring_a_missing_session_fails_cleanly() {
    hyprwake()
        .args(["restore", "no-such-session-xyz"])
        .assert()
        .failure()
        .stderr(contains("no session"));
}

#[test]
fn deleting_a_missing_session_fails_cleanly() {
    hyprwake()
        .args(["delete", "no-such-session-xyz"])
        .assert()
        .failure()
        .stderr(contains("not found"));
}
