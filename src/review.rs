use crate::policy::Policy;
use crate::portfolio::Portfolio;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub portfolio_value: f64,
    pub currency: String,
    pub policy_name: String,
    pub findings: Vec<Finding>,
    pub allocations: Vec<AllocationReview>,
    pub constraint_checks: Vec<ConstraintCheck>,
    pub data_quality: Vec<DataQualityIssue>,
    pub suggested_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub severity: Severity,
    pub category: String,
    pub message: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationReview {
    pub asset_class: String,
    pub target_percent: f64,
    pub tolerance_percent: Option<f64>,
    pub actual_percent: f64,
    pub drift_percent: f64,
    pub within_tolerance: bool,
    pub value: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConstraintCheck {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataQualityIssue {
    pub severity: Severity,
    pub position: String,
    pub issue: String,
}

impl Review {
    pub fn from_portfolio_and_policy(
        portfolio: &Portfolio,
        policy: &Policy,
        currency: &str,
    ) -> Self {
        let portfolio_value = portfolio.get_total_value();
        let actual_allocation = portfolio.get_allocation();

        let mut allocations = Vec::new();
        let mut findings = Vec::new();
        let mut suggested_actions = Vec::new();

        // Review each policy allocation target
        for target in &policy.allocations {
            let actual_percent = actual_allocation
                .get(&target.asset_class)
                .copied()
                .unwrap_or(0.0);

            let drift = actual_percent - target.target_percent;
            let tolerance = target.tolerance_percent.unwrap_or(5.0);
            let within_tolerance = drift.abs() <= tolerance;

            let value = portfolio_value * actual_percent / 100.0;

            allocations.push(AllocationReview {
                asset_class: target.asset_class.clone(),
                target_percent: target.target_percent,
                tolerance_percent: target.tolerance_percent,
                actual_percent,
                drift_percent: drift,
                within_tolerance,
                value,
            });

            if !within_tolerance {
                let severity = if drift.abs() > tolerance * 2.0 {
                    Severity::Critical
                } else {
                    Severity::Warning
                };

                let direction = if drift > 0.0 { "above" } else { "below" };

                findings.push(Finding {
                    severity,
                    category: "Allocation Drift".to_string(),
                    message: format!(
                        "{} allocation is {} target: {:.1}% vs {:.1}%",
                        target.asset_class, direction, actual_percent, target.target_percent
                    ),
                    detail: Some(format!(
                        "Drift: {:.1}% (tolerance: ±{:.1}%)",
                        drift, tolerance
                    )),
                });

                suggested_actions.push(format!(
                    "Rebalance {}: currently {:.1}%, target {:.1}% (±{:.1}%)",
                    target.asset_class, actual_percent, target.target_percent, tolerance
                ));
            }
        }

        // Check for asset classes in portfolio but not in policy
        for (asset_class, actual_percent) in &actual_allocation {
            if !policy
                .allocations
                .iter()
                .any(|a| a.asset_class.eq_ignore_ascii_case(asset_class))
            {
                findings.push(Finding {
                    severity: Severity::Warning,
                    category: "Unknown Asset Class".to_string(),
                    message: format!(
                        "Asset class '{}' ({:.1}%) is not defined in policy",
                        asset_class, actual_percent
                    ),
                    detail: Some(
                        "Consider adding this asset class to your policy or reclassifying positions"
                            .to_string(),
                    ),
                });
            }
        }

        let constraint_checks = check_constraints(portfolio, policy, portfolio_value);
        for check in &constraint_checks {
            if !check.passed {
                findings.push(Finding {
                    severity: Severity::Warning,
                    category: "Constraint Violation".to_string(),
                    message: check.message.clone(),
                    detail: None,
                });
            }
        }

        let data_quality = check_data_quality(portfolio);
        for issue in &data_quality {
            findings.push(Finding {
                severity: issue.severity.clone(),
                category: "Data Quality".to_string(),
                message: format!("{}: {}", issue.position, issue.issue),
                detail: None,
            });
        }

        // Sort findings by severity
        findings.sort_by(|a, b| {
            let severity_order = |s: &Severity| match s {
                Severity::Critical => 0,
                Severity::Warning => 1,
                Severity::Info => 2,
            };
            severity_order(&a.severity).cmp(&severity_order(&b.severity))
        });

        if findings.is_empty() {
            findings.push(Finding {
                severity: Severity::Info,
                category: "Policy Alignment".to_string(),
                message: "Portfolio is aligned with policy. No issues found.".to_string(),
                detail: None,
            });
        }

        suggested_actions.push(
            "Run 'portfolio_rs context --format json' for detailed position data".to_string(),
        );

        Self {
            portfolio_value,
            currency: currency.to_string(),
            policy_name: policy.name.clone(),
            findings,
            allocations,
            constraint_checks,
            data_quality,
            suggested_actions,
        }
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::new();

        out.push_str("# Portfolio Review\n\n");
        out.push_str(&format!(
            "Portfolio Value: {:.2} {}\n",
            self.portfolio_value, self.currency
        ));
        out.push_str(&format!("Policy: {}\n\n", self.policy_name));

        out.push_str("## Findings\n\n");
        if self.findings.is_empty() {
            out.push_str("No findings.\n");
        } else {
            for finding in &self.findings {
                let icon = match finding.severity {
                    Severity::Critical => "❌",
                    Severity::Warning => "⚠️",
                    Severity::Info => "✅",
                };
                out.push_str(&format!(
                    "{} **{}** — {}\n",
                    icon, finding.category, finding.message
                ));
                if let Some(detail) = &finding.detail {
                    out.push_str(&format!("  > {}\n", detail));
                }
            }
        }

        out.push_str("\n## Allocation Review\n\n");
        out.push_str("| Asset Class | Target | Actual | Drift | Status |\n");
        out.push_str("|-------------|--------|--------|-------|--------|\n");
        for alloc in &self.allocations {
            let status = if alloc.within_tolerance { "✅" } else { "❌" };
            out.push_str(&format!(
                "| {} | {:.1}% | {:.1}% | {:+.1}% | {} |\n",
                alloc.asset_class,
                alloc.target_percent,
                alloc.actual_percent,
                alloc.drift_percent,
                status
            ));
        }

        out.push_str("\n## Constraint Checks\n\n");
        for check in &self.constraint_checks {
            let icon = if check.passed { "✅" } else { "❌" };
            out.push_str(&format!("{} {}\n", icon, check.message));
        }

        if !self.data_quality.is_empty() {
            out.push_str("\n## Data Quality\n\n");
            for issue in &self.data_quality {
                let icon = match issue.severity {
                    Severity::Critical => "❌",
                    Severity::Warning => "⚠️",
                    Severity::Info => "ℹ️",
                };
                out.push_str(&format!(
                    "{} **{}**: {}\n",
                    icon, issue.position, issue.issue
                ));
            }
        }

        out.push_str("\n## Suggested Actions\n\n");
        for action in &self.suggested_actions {
            out.push_str(&format!("- {}\n", action));
        }

        out
    }

    pub fn to_json(&self) -> eyre::Result<String> {
        serde_json::to_string_pretty(self).map_err(Into::into)
    }
}

fn check_constraints(
    portfolio: &Portfolio,
    policy: &Policy,
    total_value: f64,
) -> Vec<ConstraintCheck> {
    let mut checks = Vec::new();

    // Cash buffer check
    if let Some(min_cash_amount) = policy.constraints.minimum_cash_amount {
        let cash_value: f64 = portfolio
            .positions
            .iter()
            .filter(|p| {
                p.get_ticker().is_none() && p.get_asset_class().eq_ignore_ascii_case("cash")
            })
            .map(|p| p.market_value())
            .sum();

        let passed = cash_value >= min_cash_amount;
        checks.push(ConstraintCheck {
            name: "Minimum Cash Amount".to_string(),
            passed,
            message: format!(
                "Cash buffer: {:.2} {} (minimum: {:.2} {})",
                cash_value, policy.base_currency, min_cash_amount, policy.base_currency
            ),
        });
    }

    // Single position concentration
    if let Some(limit) = policy.constraints.single_position_limit_percent {
        for position in &portfolio.positions {
            let weight = if total_value > 0.0 {
                position.market_value() / total_value * 100.0
            } else {
                0.0
            };

            if weight > limit {
                checks.push(ConstraintCheck {
                    name: "Single Position Limit".to_string(),
                    passed: false,
                    message: format!(
                        "{} is {:.1}% of portfolio (limit: {:.1}%)",
                        position.get_name(),
                        weight,
                        limit
                    ),
                });
            }
        }
    }

    // Asset class concentration
    if let Some(limit) = policy.constraints.asset_class_limit_percent {
        let allocation = portfolio.get_allocation();
        for (asset_class, percent) in allocation {
            if percent > limit {
                checks.push(ConstraintCheck {
                    name: "Asset Class Limit".to_string(),
                    passed: false,
                    message: format!(
                        "{} is {:.1}% of portfolio (limit: {:.1}%)",
                        asset_class, percent, limit
                    ),
                });
            }
        }
    }

    checks
}

fn check_data_quality(portfolio: &Portfolio) -> Vec<DataQualityIssue> {
    let mut issues = Vec::new();

    for position in &portfolio.positions {
        // Missing cost basis for securities
        if position.get_ticker().is_some() && position.total_invested().is_none() {
            issues.push(DataQualityIssue {
                severity: Severity::Warning,
                position: position.get_name().to_string(),
                issue: "Missing cost basis — add Purchase entries for PnL tracking".to_string(),
            });
        }

        // No ticker for non-cash positions
        if position.get_ticker().is_none()
            && !position.get_asset_class().eq_ignore_ascii_case("cash")
        {
            issues.push(DataQualityIssue {
                severity: Severity::Warning,
                position: position.get_name().to_string(),
                issue: "No ticker — live pricing unavailable".to_string(),
            });
        }

        // Missing market price
        if position.get_ticker().is_some() && position.market_price() <= 0.0 {
            issues.push(DataQualityIssue {
                severity: Severity::Warning,
                position: position.get_name().to_string(),
                issue: "No market price — quote lookup may have failed".to_string(),
            });
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{AllocationTarget, Constraints, Policy, RiskProfile};
    use crate::portfolio::Portfolio;

    fn create_test_policy() -> Policy {
        Policy {
            version: "1.0".to_string(),
            name: "Test Policy".to_string(),
            base_currency: "EUR".to_string(),
            time_horizon_years: 10,
            risk_profile: RiskProfile::Moderate,
            constraints: Constraints {
                minimum_cash_months: Some(6),
                minimum_cash_amount: Some(10000.0),
                single_position_limit_percent: Some(50.0),
                asset_class_limit_percent: Some(80.0),
            },
            allocations: vec![
                AllocationTarget {
                    asset_class: "Stocks".to_string(),
                    target_percent: 60.0,
                    tolerance_percent: Some(10.0),
                },
                AllocationTarget {
                    asset_class: "Cash".to_string(),
                    target_percent: 40.0,
                    tolerance_percent: Some(10.0),
                },
            ],
        }
    }

    #[test]
    fn test_review_empty_portfolio() {
        let portfolio = Portfolio::new();
        let policy = create_test_policy();
        let review = Review::from_portfolio_and_policy(&portfolio, &policy, "EUR");

        assert_eq!(review.portfolio_value, 0.0);
        assert!(!review.findings.is_empty());
    }

    #[test]
    fn test_review_to_markdown_contains_sections() {
        let portfolio = Portfolio::new();
        let policy = create_test_policy();
        let review = Review::from_portfolio_and_policy(&portfolio, &policy, "EUR");
        let md = review.to_markdown();

        assert!(md.contains("# Portfolio Review"));
        assert!(md.contains("## Findings"));
        assert!(md.contains("## Allocation Review"));
        assert!(md.contains("## Constraint Checks"));
        assert!(md.contains("## Suggested Actions"));
    }

    #[test]
    fn test_review_to_json_valid() {
        let portfolio = Portfolio::new();
        let policy = create_test_policy();
        let review = Review::from_portfolio_and_policy(&portfolio, &policy, "EUR");
        let json = review.to_json().unwrap();

        assert!(json.contains("portfolioValue"));
        assert!(json.contains("findings"));
        assert!(json.contains("allocations"));
    }
}
