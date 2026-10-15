use assert_cmd::Command;
use predicates::str::contains;

fn hyprwake() -> Command {
    // Cargo points this at the freshly built binary for integration tests.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hyprwake"));
    // Never the user's own config, sessions or log: a test that resolved a
    // session name or logged would otherwise do it in their home.
    let scratch = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-xdg");
    cmd.env("XDG_CONFIG_HOME", scratch.join("config"))
        .env("XDG_DATA_HOME", scratch.join("share"))
        .env("XDG_STATE_HOME", scratch.join("state"));
    cmd
}

#[test]
fn help_lists_the_main_commands() {
    hyprwake()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("save"))
        .stdout(contains("restore"))
        .stdout(contains("diff"))
        .stdout(contains("watch"))
        .stdout(contains("status"))
        .stdout(contains("providers"))
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

/// A hyprwake whose config directory is `dir`, so a test controls the
/// providers it sees without touching the real `~/.config`.
fn hyprwake_with_config(dir: &tempfile::TempDir, config: &str) -> Command {
    let conf = dir.path().join("hyprwake");
    std::fs::create_dir_all(&conf).unwrap();
    std::fs::write(conf.join("config.toml"), config).unwrap();
    let mut cmd = hyprwake();
    cmd.env("XDG_CONFIG_HOME", dir.path());
    cmd
}

#[test]
fn providers_lists_the_builtins_and_the_fallback() {
    let dir = tempfile::TempDir::new().unwrap();
    hyprwake_with_config(&dir, "")
        .arg("providers")
        .assert()
        .success()
        .stdout(contains("org.omarchy.terminal"))
        .stdout(contains("generic"))
        .stdout(contains("(any other class)"))
        .stdout(contains("opencode (by lookup)"));
}

#[test]
fn providers_explains_one_class() {
    let dir = tempfile::TempDir::new().unwrap();
    let config =
        "[providers.jet]\nkind = \"command\"\nclasses = [\"jetbrains-*\"]\nspawn = false\n";
    hyprwake_with_config(&dir, config)
        .args(["providers", "jetbrains-idea"])
        .assert()
        .success()
        .stdout(contains("jetbrains-idea → jet (command, [providers])"))
        .stdout(contains("matches the pattern \"jetbrains-*\""))
        .stdout(contains("never launched"));
}

#[test]
fn providers_reports_declaration_problems() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = "[providers.a]\nkind = \"command\"\nclasses = [\"x\"]\n\
                  [providers.b]\nkind = \"command\"\nclasses = [\"x\"]\n";
    hyprwake_with_config(&dir, config)
        .arg("providers")
        .assert()
        .success()
        .stdout(contains("Problems:"))
        .stdout(contains("claimed by both"));
}

#[test]
fn topology_accepts_only_known_values() {
    hyprwake()
        .args(["restore", "--topology", "sideways", "--dry-run"])
        .assert()
        .failure()
        .stderr(contains("adapt"))
        .stderr(contains("saved"));
}
