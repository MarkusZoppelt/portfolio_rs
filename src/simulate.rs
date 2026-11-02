//! Rebalancing simulation against an investment policy.
//!
//! Produces what-if scenarios (full rebalance, conservative partial
//! rebalance) without executing any trades.

use serde::{Deserialize, Serialize};

use crate::money::{dec, to_f64, Decimal};
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
            let amount = to_f64(dec(drift.abs()) * dec(portfolio_value) / Decimal::ONE_HUNDRED);
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
            let amount = to_f64(dec(drift.abs()) * dec(portfolio_value) / dec(200.0));
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

    use crate::policy::{AllocationTarget, Constraints, RiskProfile};
    use crate::position::{PortfolioPosition, PriceStatus};

    /// Stocks 50% (±10), Cash 50% (default ±5), Gold 0%.
    fn half_half_policy() -> Policy {
        let target = |class: &str, percent, tolerance| AllocationTarget {
            asset_class: class.to_string(),
            target_percent: percent,
            tolerance_percent: tolerance,
        };
        Policy {
            version: "1.0".to_string(),
            name: "Half Half".to_string(),
            base_currency: "EUR".to_string(),
            time_horizon_years: 10,
            risk_profile: RiskProfile::Moderate,
            constraints: Constraints::default(),
            allocations: vec![
                target("Stocks", 50.0, Some(10.0)),
                target("Cash", 50.0, None),
                target("Gold", 0.0, None),
            ],
        }
    }

    fn portfolio(stocks: f64, cash: f64) -> Portfolio {
        let mut portfolio = Portfolio::new();
        portfolio.add_position(PortfolioPosition::for_test(
            Some("FAKESTOCK"),
            "Stocks",
            1.0,
            stocks,
            PriceStatus::Live,
        ));
        portfolio.add_position(PortfolioPosition::for_test(
            None,
            "Cash",
            cash,
            0.0,
            PriceStatus::Live,
        ));
        portfolio
    }

    #[test]
    fn test_drifted_portfolio_gets_full_and_half_trades() {
        let sim = simulate_rebalance(&portfolio(800.0, 200.0), &half_half_policy(), "EUR");
        assert_eq!(sim.portfolio_value, 1000.0);
        assert_eq!(sim.policy_name, "Half Half");

        let full = &sim.scenarios[0];
        let trades: Vec<_> = full
            .trades
            .iter()
            .map(|t| (t.asset_class.as_str(), t.action.as_str(), t.amount))
            .collect();
        // Gold is on target, so it needs no trade.
        assert_eq!(
            trades,
            [("Stocks", "Reduce", 300.0), ("Cash", "Increase", 300.0)]
        );
        assert_eq!(full.new_allocation.len(), 3);

        let partial = &sim.scenarios[1];
        let trades: Vec<_> = partial
            .trades
            .iter()
            .map(|t| {
                (
                    t.asset_class.as_str(),
                    t.action.as_str(),
                    t.amount,
                    t.percent_of_portfolio,
                )
            })
            .collect();
        assert_eq!(
            trades,
            [
                ("Stocks", "Reduce", 150.0, 15.0),
                ("Cash", "Increase", 150.0, 15.0)
            ]
        );

        let md = sim.to_markdown();
        assert!(md.contains("Portfolio Value: 1000.00 EUR\n"), "{md}");
        assert!(md.contains("Policy: Half Half\n"), "{md}");
        assert!(
            md.contains("## Scenario 1: Full Rebalance to Targets"),
            "{md}"
        );
        assert!(
            md.contains("| Stocks | Reduce | 300.00 EUR | 30.0% |"),
            "{md}"
        );
        assert!(
            md.contains("| Cash | Increase | 150.00 EUR | 15.0% |"),
            "{md}"
        );
        assert!(!md.contains("No trades needed."), "{md}");
        assert!(md.ends_with("No trades have been executed.\n"), "{md}");
    }

    #[test]
    fn test_small_drift_inside_tolerance_needs_no_trades() {
        // 54/46: drift beyond 0.5 trades in full, but stays inside tolerance.
        let sim = simulate_rebalance(&portfolio(540.0, 460.0), &half_half_policy(), "EUR");
        assert_eq!(sim.scenarios[0].trades.len(), 2);
        assert!(sim.scenarios[1].trades.is_empty());

        let sim = simulate_rebalance(&portfolio(500.0, 500.0), &half_half_policy(), "EUR");
        assert!(sim.scenarios.iter().all(|s| s.trades.is_empty()));
        let md = sim.to_markdown();
        assert_eq!(md.matches("No trades needed.").count(), 2, "{md}");
    }
}
