use crate::policy::Policy;
use crate::review::Review;
use crate::services::portfolio_loader::{create_live_portfolio_with_logging, load_portfolio_file};

/// Generate the Markdown body of a structured decision draft.
pub async fn generate_decision_draft(
    filename: &str,
    policy_file: &str,
    title: &str,
    currency: &str,
) -> String {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut content = format!(
        "# Decision: {}\n\nDate: {}\nStatus: Proposed\n\n",
        title, date
    );

    if !filename.is_empty() {
        if let Ok(positions_str) = load_portfolio_file(filename) {
            let (portfolio, _network_status) =
                create_live_portfolio_with_logging(positions_str, true).await;

            if let Ok(policy) = Policy::from_file(policy_file) {
                let review = Review::from_portfolio_and_policy(&portfolio, &policy, currency);

                content.push_str("## Context\n\n");
                content.push_str(&format!(
                    "Portfolio value: {:.2} {}\n\n",
                    review.portfolio_value, review.currency
                ));

                if !review.findings.is_empty() {
                    content.push_str("Key findings from review:\n\n");
                    for finding in &review.findings {
                        content.push_str(&format!("- {}: {}\n", finding.category, finding.message));
                    }
                    content.push('\n');
                }
            }
        }
    }

    content.push_str("## Decision\n\n");
    content.push_str("## Rationale\n\n");
    content.push_str("## Risks\n\n");
    content.push_str("## Review Date\n\n");
    content
}
