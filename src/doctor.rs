use crate::policy::Policy;
use eyre::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum HealthStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheck {
    pub name: String,
    pub status: HealthStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceHealth {
    pub dir: String,
    pub checks: Vec<HealthCheck>,
    pub issue_count: usize,
    pub warning_count: usize,
}

/// `.gitignore` patterns that must cover backups, temp files, the database
/// and the performance snapshots (history dir, files and their temp files).
pub const PRIVATE_ARTIFACT_PATTERNS: [&str; 7] = [
    "positions.json.bak",
    ".positions.json.tmp-*",
    ".positions.json.quotes-cache.json",
    "database/",
    "portfolio/history/",
    "snapshots.jsonl",
    ".snapshots.jsonl.tmp-*",
];

/// Warn when private artifacts are not ignored, or are already tracked by git.
fn private_artifacts_check(dir: &Path) -> (HealthStatus, String) {
    let ignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap_or_default();
    let lines: Vec<&str> = ignore.lines().map(str::trim).collect();
    let missing: Vec<&str> = PRIVATE_ARTIFACT_PATTERNS
        .iter()
        .copied()
        .filter(|p| !lines.iter().any(|l| l.trim_start_matches('/') == *p))
        .collect();
    let tracked = tracked_private_artifacts(dir);

    let mut problems = Vec::new();
    if !missing.is_empty() {
        problems.push(format!("not in .gitignore: {}", missing.join(", ")));
    }
    if !tracked.is_empty() {
        problems.push(format!(
            "tracked by git (run `git rm --cached`): {}",
            tracked.join(", ")
        ));
    }
    if problems.is_empty() {
        (
            HealthStatus::Ok,
            "Backups, temp files and database/ are ignored and untracked".to_string(),
        )
    } else {
        (
            HealthStatus::Warning,
            format!("Private artifacts {}", problems.join("; ")),
        )
    }
}

/// Files under `dir` that git tracks and should not. Empty if git is unavailable.
fn tracked_private_artifacts(dir: &Path) -> Vec<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "ls-files",
            "--",
            "positions.json",
            "positions.json.bak",
            ".positions.json.tmp-*",
            ".positions.json.quotes-cache.json",
            "database",
            "portfolio/history",
            ":(glob)**/snapshots.jsonl",
            ":(glob)**/.snapshots.jsonl.tmp-*",
        ])
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Check a workspace directory for common health issues.
pub fn check_workspace(dir: &str) -> Result<WorkspaceHealth> {
    let mut checks = Vec::new();
    let mut issue_count = 0;
    let mut warning_count = 0;

    // Check policy file
    let policy_path = Path::new(dir).join("portfolio/policy.toml");
    if policy_path.exists() {
        match Policy::from_file(&policy_path.to_string_lossy()) {
            Ok(_) => checks.push(HealthCheck {
                name: "Policy file".to_string(),
                status: HealthStatus::Ok,
                message: format!("Policy file is valid: {}", policy_path.display()),
            }),
            Err(e) => {
                checks.push(HealthCheck {
                    name: "Policy file".to_string(),
                    status: HealthStatus::Error,
                    message: format!("Policy file is invalid: {e}"),
                });
                issue_count += 1;
            }
        }
    } else {
        checks.push(HealthCheck {
            name: "Policy file".to_string(),
            status: HealthStatus::Warning,
            message: format!("Policy file not found: {}", policy_path.display()),
        });
        warning_count += 1;
    }

    // Check narrative policy
    let narrative_policy = Path::new(dir).join("INVESTMENT_POLICY.md");
    if narrative_policy.exists() {
        checks.push(HealthCheck {
            name: "Narrative policy".to_string(),
            status: HealthStatus::Ok,
            message: format!("Narrative policy found: {}", narrative_policy.display()),
        });
    } else {
        checks.push(HealthCheck {
            name: "Narrative policy".to_string(),
            status: HealthStatus::Warning,
            message: format!("Narrative policy not found: {}", narrative_policy.display()),
        });
        warning_count += 1;
    }

    // Check workspace directories
    let required_dirs = [
        "portfolio/diary",
        "portfolio/decisions",
        "portfolio/theses",
        "portfolio/reports",
    ];

    for d in &required_dirs {
        let path = Path::new(dir).join(d);
        if path.is_dir() {
            checks.push(HealthCheck {
                name: format!("Directory: {d}"),
                status: HealthStatus::Ok,
                message: format!("Directory exists: {}", path.display()),
            });
        } else {
            checks.push(HealthCheck {
                name: format!("Directory: {d}"),
                status: HealthStatus::Warning,
                message: format!("Directory missing: {}", path.display()),
            });
            warning_count += 1;
        }
    }

    // Check watchlist
    let watchlist = Path::new(dir).join("portfolio/watchlist.json");
    if watchlist.exists() {
        checks.push(HealthCheck {
            name: "Watchlist".to_string(),
            status: HealthStatus::Ok,
            message: format!("Watchlist found: {}", watchlist.display()),
        });
    } else {
        checks.push(HealthCheck {
            name: "Watchlist".to_string(),
            status: HealthStatus::Warning,
            message: format!("Watchlist not found: {}", watchlist.display()),
        });
        warning_count += 1;
    }

    // Check gitignore safety (in the workspace being diagnosed, not the CWD)
    let gitignore = Path::new(dir).join(".gitignore");
    if gitignore.exists() {
        if let Ok(content) = std::fs::read_to_string(&gitignore) {
            if content.contains("/portfolio/") || content.contains("*.gpg") {
                checks.push(HealthCheck {
                    name: "Gitignore safety".to_string(),
                    status: HealthStatus::Ok,
                    message: ".gitignore protects private workspace data".to_string(),
                });
            } else {
                checks.push(HealthCheck {
                    name: "Gitignore safety".to_string(),
                    status: HealthStatus::Warning,
                    message: ".gitignore may not protect private workspace data".to_string(),
                });
                warning_count += 1;
            }
        }
    } else {
        checks.push(HealthCheck {
            name: "Gitignore safety".to_string(),
            status: HealthStatus::Warning,
            message: "No .gitignore found".to_string(),
        });
        warning_count += 1;
    }

    // Backups, temp files and the local database hold private data too.
    let (status, message) = private_artifacts_check(Path::new(dir));
    if status == HealthStatus::Warning {
        warning_count += 1;
    }
    checks.push(HealthCheck {
        name: "Private artifacts".to_string(),
        status,
        message,
    });

    // Check for unencrypted sensitive files
    let positions = Path::new(dir).join("positions.json");
    if positions.exists() {
        checks.push(HealthCheck {
            name: "Encryption".to_string(),
            status: HealthStatus::Warning,
            message: format!(
                "Unencrypted positions file found: {}. Consider encrypting with: gpg -c {}",
                positions.display(),
                positions.display()
            ),
        });
        warning_count += 1;
    }

    Ok(WorkspaceHealth {
        dir: dir.to_string(),
        checks,
        issue_count,
        warning_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn test_private_artifacts_must_be_ignored() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "positions.json\n").unwrap();
        let (status, message) = private_artifacts_check(tmp.path());
        assert_eq!(status, HealthStatus::Warning);
        assert!(message.contains("positions.json.bak"));
        assert!(message.contains("database/"));
        assert!(message.contains("portfolio/history/"), "{message}");
        assert!(message.contains(".snapshots.jsonl.tmp-*"), "{message}");

        let all = PRIVATE_ARTIFACT_PATTERNS.join("\n");
        std::fs::write(tmp.path().join(".gitignore"), all).unwrap();
        assert_eq!(private_artifacts_check(tmp.path()).0, HealthStatus::Ok);
    }

    #[test]
    fn test_tracked_private_artifacts_warn() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        // git may be missing (e.g. sandboxed build); the check is then a no-op.
        if git(dir, &["init", "-q"]) {
            assert_tracked_artifacts_warn(dir);
        }
    }

    fn assert_tracked_artifacts_warn(dir: &Path) {
        std::fs::write(dir.join(".gitignore"), PRIVATE_ARTIFACT_PATTERNS.join("\n")).unwrap();
        std::fs::write(dir.join("positions.json.bak"), "[]").unwrap();
        assert!(git(dir, &["add", "-f", "positions.json.bak"]));
        let (status, message) = private_artifacts_check(dir);
        assert_eq!(status, HealthStatus::Warning);
        assert!(message.contains("tracked by git"), "{message}");
        assert!(message.contains("positions.json.bak"));

        std::fs::create_dir_all(dir.join("portfolio/history/main")).unwrap();
        std::fs::write(dir.join("portfolio/history/main/snapshots.jsonl"), "").unwrap();
        assert!(git(
            dir,
            &["add", "-f", "portfolio/history/main/snapshots.jsonl"]
        ));
        let (_, message) = private_artifacts_check(dir);
        assert!(message.contains("snapshots.jsonl"), "{message}");
    }

    /// Finance B6: the workspace template satisfies the check.
    #[test]
    fn test_workspace_template_ignores_snapshots() {
        let tmp = tempfile::TempDir::new().unwrap();
        crate::workspace::init_workspace(tmp.path().to_str().unwrap(), false).unwrap();
        assert_eq!(private_artifacts_check(tmp.path()).0, HealthStatus::Ok);
    }

    fn statuses(health: &WorkspaceHealth) -> Vec<(&str, HealthStatus)> {
        health
            .checks
            .iter()
            .map(|c| (c.name.as_str(), c.status))
            .collect()
    }

    #[test]
    fn test_empty_dir_warns_about_everything() {
        use HealthStatus::Warning;
        let tmp = tempfile::TempDir::new().unwrap();
        let health = check_workspace(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(
            statuses(&health),
            [
                ("Policy file", Warning),
                ("Narrative policy", Warning),
                ("Directory: portfolio/diary", Warning),
                ("Directory: portfolio/decisions", Warning),
                ("Directory: portfolio/theses", Warning),
                ("Directory: portfolio/reports", Warning),
                ("Watchlist", Warning),
                ("Gitignore safety", Warning),
                ("Private artifacts", Warning),
            ]
        );
        assert_eq!((health.issue_count, health.warning_count), (0, 9));
        assert_eq!(health.checks[7].message, "No .gitignore found");
    }

    #[test]
    fn test_initialized_workspace_with_policy_is_healthy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().to_str().unwrap();
        crate::workspace::init_workspace(dir, false).unwrap();
        crate::commands::policy::policy_init("balanced-growth", dir, false).unwrap();
        let health = check_workspace(dir).unwrap();
        assert!(
            health.checks.iter().all(|c| c.status == HealthStatus::Ok),
            "{:?}",
            health.checks
        );
        assert_eq!((health.issue_count, health.warning_count), (0, 0));
    }

    #[test]
    fn test_invalid_policy_weak_gitignore_and_plain_positions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().to_str().unwrap();
        crate::workspace::init_workspace(dir, false).unwrap();
        std::fs::write(tmp.path().join("portfolio/policy.toml"), "not = [toml").unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::write(tmp.path().join("positions.json"), "[]").unwrap();

        let health = check_workspace(dir).unwrap();
        let check = |name: &str| health.checks.iter().find(|c| c.name == name).unwrap();
        assert_eq!(check("Policy file").status, HealthStatus::Error);
        assert!(check("Policy file")
            .message
            .starts_with("Policy file is invalid: "));
        assert_eq!(check("Gitignore safety").status, HealthStatus::Warning);
        assert_eq!(
            check("Gitignore safety").message,
            ".gitignore may not protect private workspace data"
        );
        assert_eq!(check("Private artifacts").status, HealthStatus::Warning);
        assert_eq!(check("Encryption").status, HealthStatus::Warning);
        assert!(check("Encryption").message.contains("gpg -c"));
        assert_eq!((health.issue_count, health.warning_count), (1, 3));
    }

    #[test]
    fn test_unreadable_gitignore_is_not_judged() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".gitignore")).unwrap();
        let health = check_workspace(tmp.path().to_str().unwrap()).unwrap();
        assert!(health.checks.iter().all(|c| c.name != "Gitignore safety"));
        // The private-artifact check still warns: nothing is ignored.
        let artifacts = health
            .checks
            .iter()
            .find(|c| c.name == "Private artifacts")
            .unwrap();
        assert_eq!(artifacts.status, HealthStatus::Warning);
    }
}
