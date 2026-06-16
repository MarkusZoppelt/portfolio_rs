use crate::policy::Policy;
use crate::review::Review;
use crate::services::portfolio_loader::{create_live_portfolio_with_logging, load_portfolio_file};

/// Generate the Markdown body of a weekly portfolio report.
pub async fn generate_markdown_report(
    filename: &str,
    policy_file: &str,
    date: &str,
    currency: &str,
) -> String {
    let mut content = format!("# Weekly Portfolio Report\n\nDate: {}\n\n", date);

    if !filename.is_empty() {
        if let Ok(positions_str) = load_portfolio_file(filename) {
            let (portfolio, network_status) =
                create_live_portfolio_with_logging(positions_str, true).await;

            content.push_str("## Portfolio Summary\n\n");
            content.push_str(&format!(
                "- Total Value: {:.2} {}\n",
                portfolio.get_total_value(),
                currency
            ));
            content.push_str(&format!("- Network Status: {:?}\n\n", network_status));

            if let Ok(policy) = Policy::from_file(policy_file) {
                let review = Review::from_portfolio_and_policy(&portfolio, &policy, currency);

                content.push_str("## Allocation vs Target\n\n");
                content.push_str("| Asset Class | Target | Actual | Drift | Status |\n");
                content.push_str("|-------------|--------|--------|-------|--------|\n");
                for alloc in &review.allocations {
                    let status = if alloc.within_tolerance { "✅" } else { "❌" };
                    content.push_str(&format!(
                        "| {} | {:.1}% | {:.1}% | {:+.1}% | {} |\n",
                        alloc.asset_class,
                        alloc.target_percent,
                        alloc.actual_percent,
                        alloc.drift_percent,
                        status
                    ));
                }
                content.push('\n');

                if !review.findings.is_empty() {
                    content.push_str("## Findings\n\n");
                    for finding in &review.findings {
                        content.push_str(&format!(
                            "- **{}**: {}\n",
                            finding.category, finding.message
                        ));
                    }
                    content.push('\n');
                }

                if !review.suggested_actions.is_empty() {
                    content.push_str("## Suggested Actions\n\n");
                    for action in &review.suggested_actions {
                        content.push_str(&format!("- {}\n", action));
                    }
                    content.push('\n');
                }
            }
        }
    }

    content.push_str("## Notes\n\n");
    content
}
