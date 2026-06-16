//! Rebalancing simulation against an investment policy.
//!
//! Produces what-if scenarios (full rebalance, conservative partial
//! rebalance) without executing any trades.

use serde::{Deserialize, Serialize};

use crate::policy::Policy;
use crate::portfolio::Portfolio;

/// The result of simulating rebalancing scenarios for a portfolio.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebalanceSimulation {
    /// Total portfolio value in `currency`.
    pub portfolio_value: f64,
    /// Currency the amounts are denominated in.
    pub currency: String,
    /// Name of the policy the simulation ran against.
    pub policy_name: String,
    /// Simulated scenarios, from most to least aggressive.
    pub scenarios: Vec<RebalanceScenario>,
}

/// A single rebalancing scenario with its proposed trades.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebalanceScenario {
    pub name: String,
    pub description: String,
    pub trades: Vec<Trade>,
    pub new_allocation: Vec<AllocationItem>,
    pub cash_impact: f64,
}

/// A proposed (never executed) trade.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trade {
    pub asset_class: String,
    /// `"Increase"` or `"Reduce"`.
    pub action: String,
    pub amount: f64,
    pub percent_of_portfolio: f64,
}

/// An asset class share of the portfolio after a scenario is applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllocationItem {
    pub asset_class: String,
    pub percent: f64,
}

/// Simulate rebalancing scenarios for `portfolio` against `policy`.
///
/// This is purely analytical: no trades are executed.
pub fn simulate_rebalance(
    portfolio: &Portfolio,
    policy: &Policy,
    currency: &str,
) -> RebalanceSimulation {
    let portfolio_value = portfolio.get_total_value();
    let actual_allocation = portfolio.get_allocation();
    let mut scenarios = Vec::new();

    // Scenario 1: Full rebalance to targets
    let mut trades = Vec::new();
    let mut new_alloc = Vec::new();
    let cash_impact = 0.0;

    for target in &policy.allocations {
        let actual_percent = actual_allocation
            .get(&target.asset_class)
            .copied()
            .unwrap_or(0.0);
        let drift = actual_percent - target.target_percent;

        if drift.abs() > 0.5 {
            let action = if drift > 0.0 { "Reduce" } else { "Increase" };
            let amount = (drift.abs() / 100.0) * portfolio_value;
            trades.push(Trade {
                asset_class: target.asset_class.clone(),
                action: action.to_string(),
                amount,
                percent_of_portfolio: drift.abs(),
            });
        }

        new_alloc.push(AllocationItem {
            asset_class: target.asset_class.clone(),
            percent: target.target_percent,
        });
    }

    scenarios.push(RebalanceScenario {
        name: "Full Rebalance to Targets".to_string(),
        description: "Rebalance all asset classes to policy targets.".to_string(),
        trades,
        new_allocation: new_alloc,
        cash_impact,
    });

    // Scenario 2: Conservative partial rebalance
    let mut trades = Vec::new();
    for target in &policy.allocations {
        let actual_percent = actual_allocation
            .get(&target.asset_class)
            .copied()
            .unwrap_or(0.0);
        let drift = actual_percent - target.target_percent;
        let tolerance = target.tolerance_percent.unwrap_or(5.0);

        // Only trade if outside tolerance
        if drift.abs() > tolerance {
            let action = if drift > 0.0 { "Reduce" } else { "Increase" };
            // Only rebalance half the drift
            let amount = (drift.abs() / 200.0) * portfolio_value;
            trades.push(Trade {
                asset_class: target.asset_class.clone(),
                action: action.to_string(),
                amount,
                percent_of_portfolio: drift.abs() / 2.0,
            });
        }
    }

    scenarios.push(RebalanceScenario {
        name: "Conservative Partial Rebalance".to_string(),
        description: "Only rebalance positions that exceed tolerance bands, by half the drift."
            .to_string(),
        trades,
        new_allocation: Vec::new(), // Simplified
        cash_impact,
    });

    RebalanceSimulation {
        portfolio_value,
        currency: currency.to_string(),
        policy_name: policy.name.clone(),
        scenarios,
    }
}

impl RebalanceSimulation {
    /// Render the simulation as human-readable Markdown.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Rebalance Simulation\n\n");
        out.push_str(&format!(
            "Portfolio Value: {:.2} {}\n",
            self.portfolio_value, self.currency
        ));
        out.push_str(&format!("Policy: {}\n\n", self.policy_name));

        for (i, scenario) in self.scenarios.iter().enumerate() {
            out.push_str(&format!("## Scenario {}: {}\n\n", i + 1, scenario.name));
            out.push_str(&format!("{}\n\n", scenario.description));

            if scenario.trades.is_empty() {
                out.push_str("No trades needed.\n\n");
            } else {
                out.push_str("| Asset Class | Action | Amount | % of Portfolio |\n");
                out.push_str("|-------------|--------|--------|----------------|\n");
                for trade in &scenario.trades {
                    out.push_str(&format!(
                        "| {} | {} | {:.2} {} | {:.1}% |\n",
                        trade.asset_class,
                        trade.action,
                        trade.amount,
                        self.currency,
                        trade.percent_of_portfolio
                    ));
                }
                out.push('\n');
            }
        }

        out.push_str("\n> Disclaimer: This is a simulation. No trades have been executed.\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::default_balanced_growth_policy;

    #[test]
    fn test_simulation_serializes_camel_case() {
        let policy = default_balanced_growth_policy();
        let portfolio = Portfolio::new();
        let sim = simulate_rebalance(&portfolio, &policy, "EUR");
        let json = serde_json::to_string(&sim).unwrap();
        assert!(json.contains("\"portfolioValue\""));
        assert!(json.contains("\"policyName\""));
        assert!(json.contains("\"newAllocation\""));
    }

    #[test]
    fn test_empty_portfolio_suggests_increases() {
        let policy = default_balanced_growth_policy();
        let portfolio = Portfolio::new();
        let sim = simulate_rebalance(&portfolio, &policy, "EUR");
        assert_eq!(sim.scenarios.len(), 2);
        // With an empty portfolio, every target is under-allocated.
        assert!(sim.scenarios[0]
            .trades
            .iter()
            .all(|t| t.action == "Increase"));
    }
}
