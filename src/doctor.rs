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
                    message: format!("Policy file is invalid: {}", e),
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
                name: format!("Directory: {}", d),
                status: HealthStatus::Ok,
                message: format!("Directory exists: {}", path.display()),
            });
        } else {
            checks.push(HealthCheck {
                name: format!("Directory: {}", d),
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
