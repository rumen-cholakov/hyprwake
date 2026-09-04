//! Privacy-conscious diagnostic reports for support and compatibility issues.

use crate::doctor::{Check, Status};
use chrono::Utc;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct SupportBundle {
    format_version: u8,
    generated_at: String,
    hyprwake_version: &'static str,
    platform: Platform,
    checks: Vec<BundleCheck>,
}

#[derive(Serialize)]
struct Platform {
    os: &'static str,
    architecture: &'static str,
}

#[derive(Serialize)]
struct BundleCheck {
    name: String,
    status: Status,
    detail: String,
}

/// The default is a new file in the current directory so users can find it
/// immediately after being asked for a report.
pub fn default_path() -> PathBuf {
    PathBuf::from(format!(
        "hyprwake-support-{}.json",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    ))
}

/// Write a deliberately narrow report. In particular, do not add snapshots,
/// logs, argv, configuration, window titles, or working directories here:
/// those commonly carry private project names and credentials.
pub fn write_bundle(path: &Path, checks: &[Check]) -> std::io::Result<()> {
    let bundle = SupportBundle {
        format_version: 1,
        generated_at: Utc::now().to_rfc3339(),
        hyprwake_version: env!("CARGO_PKG_VERSION"),
        platform: Platform {
            os: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
        },
        checks: checks
            .iter()
            .map(|check| BundleCheck {
                name: check.name.clone(),
                status: check.status,
                detail: redact_home(&check.detail),
            })
            .collect(),
    };
    let bytes = serde_json::to_vec_pretty(&bundle)
        .expect("support bundle contains only serializable primitive data");

    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn redact_home(value: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return value.to_string();
    };
    let home = home.to_string_lossy();
    if home.is_empty() {
        value.to_string()
    } else {
        value.replace(home.as_ref(), "~")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn bundle_is_private_and_redacts_the_home_directory() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("report.json");
        let home = dirs::home_dir().unwrap();
        let checks = vec![Check {
            name: "stored".to_string(),
            status: Status::Ok,
            detail: format!("session in {}/.local/share/hyprwake", home.display()),
        }];

        write_bundle(&path, &checks).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("~/.local/share/hyprwake"));
        assert!(!content.contains(home.to_string_lossy().as_ref()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn bundle_never_overwrites_an_existing_report() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("report.json");
        std::fs::write(&path, "keep this").unwrap();
        assert_eq!(
            write_bundle(&path, &[]).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "keep this");
    }
}
