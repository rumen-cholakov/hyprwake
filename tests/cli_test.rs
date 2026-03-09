use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_cli_version() {
    Command::cargo_bin("hyprflow")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("hyprflow"));
}

#[test]
fn test_cli_help() {
    Command::cargo_bin("hyprflow")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("save"))
        .stdout(predicate::str::contains("restore"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("delete"));
}

#[test]
fn test_cli_list_empty() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("hyprflow")
        .unwrap()
        .arg("list")
        .env("XDG_DATA_HOME", tmp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No saved sessions"));
}

#[test]
fn test_cli_delete_nonexistent() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("hyprflow")
        .unwrap()
        .args(["delete", "nonexistent"])
        .env("XDG_DATA_HOME", tmp.path())
        .assert()
        .failure();
}

#[test]
fn test_cli_config_shows_paths() {
    Command::cargo_bin("hyprflow")
        .unwrap()
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("Config path"))
        .stdout(predicate::str::contains("Sessions dir"));
}

#[test]
fn test_autosave_help() {
    let mut cmd = Command::cargo_bin("hyprflow").unwrap();
    cmd.args(["autosave", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("autosave"));
}
