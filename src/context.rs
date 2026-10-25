use std::{cmp::Ordering, collections::HashMap};

use chrono::Local;
use eyre::{eyre, Result};
use serde::Serialize;

use crate::mcp::dto::{money as cents, sum_money};
use crate::money::{percent_of, to_f64, Decimal};
use crate::policy::Policy;
use crate::portfolio::Portfolio;
use crate::position::PriceStatus;

const SINGLE_POSITION_CONCENTRATION_THRESHOLD: f64 = 25.0;
const ASSET_CLASS_CONCENTRATION_THRESHOLD: f64 = 60.0;
const LOW_CASH_THRESHOLD: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextOutputFormat {
    Markdown,
    Json,
}

impl ContextOutputFormat {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_lowercase().as_str() {
            "markdown" | "md" => Ok(Self::Markdown),
            "json" => Ok(Self::Json),
            _ => Err(eyre!(
                "unsupported context format: {value}. Use 'markdown' or 'json'"
            )),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioContext {
    generated_at: String,
    currency: String,
    network_status: String,
    summary: ContextSummary,
    allocation: Vec<AllocationContext>,
    positions: Vec<PositionContext>,
    risk_flags: Vec<String>,
    data_quality_flags: Vec<String>,
    follow_up_commands: Vec<String>,
    /// False when a position has no price or FX rate: totals cover only the
    /// priced positions and their values/weights are null.
    complete: bool,
    cash_buffer: CashBuffer,
}

/// Cash against the policy's `minimum_cash_amount`, in the base currency.
/// `surplus` is `actual - minimum` (negative = shortfall); `minimum` and
/// `surplus` are null without a policy minimum.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CashBuffer {
    pub minimum: Option<f64>,
    pub actual: f64,
    pub surplus: Option<f64>,
}

impl CashBuffer {
    pub fn new(portfolio: &Portfolio, policy: Option<&Policy>) -> Self {
        let actual: Decimal = portfolio
            .positions
            .iter()
            .filter(|p| p.is_cash())
            .map(|p| p.balance_dec())
            .sum();
        let minimum = policy
            .and_then(|p| p.constraints.minimum_cash_amount)
            .and_then(Decimal::from_f64_retain);
        Self {
            minimum: minimum.map(|m| to_f64(m.round_dp(2))),
            actual: to_f64(actual.round_dp(2)),
            surplus: minimum.map(|m| to_f64((actual - m).round_dp(2))),
        }
    }
}

/// JSON money and percentages carry 2 decimals (the markdown formats them the
/// same way); internal values keep full precision for the risk flags.
fn round2<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(((v * 100.0).round() / 100.0) + 0.0)
}

fn round2_opt<S: serde::Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(v) => round2(v, s),
        None => s.serialize_none(),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextSummary {
    /// Banner first in the summary; null when every position is priced.
    incomplete: Option<String>,
    /// complete | partial | stale (cached prices or FX rates in use).
    data_status: &'static str,
    #[serde(serialize_with = "round2")]
    total_value: f64,
    #[serde(serialize_with = "round2")]
    cash_value: f64,
    #[serde(serialize_with = "round2")]
    securities_value: f64,
    /// Null when incomplete: the total misses the unpriced positions, so the
    /// cash share would be overstated.
    #[serde(serialize_with = "round2_opt")]
    cash_percent: Option<f64>,
    position_count: usize,
    largest_position: Option<String>,
    #[serde(serialize_with = "round2_opt")]
    largest_position_percent: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AllocationContext {
    asset_class: String,
    /// Null when a position in the class is unpriced.
    #[serde(serialize_with = "round2_opt")]
    value: Option<f64>,
    #[serde(serialize_with = "round2_opt")]
    percent: Option<f64>,
    position_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PositionContext {
    name: String,
    ticker: Option<String>,
    asset_class: String,
    quantity: f64,
    /// Latest price in `priceCurrency` (the quote currency, not the base).
    #[serde(serialize_with = "round2_opt")]
    price: Option<f64>,
    price_currency: Option<String>,
    /// Provider timestamp of `price` (RFC 3339); null when not reported.
    price_as_of: Option<String>,
    /// True when the price or its FX rate came from the last-known-price cache.
    is_stale: bool,
    /// Base-currency value; null when the price or FX rate is unknown.
    #[serde(serialize_with = "round2_opt")]
    value: Option<f64>,
    #[serde(serialize_with = "round2_opt")]
    weight_percent: Option<f64>,
    /// Average cost in `priceCurrency`.
    #[serde(serialize_with = "round2_opt")]
    average_cost: Option<f64>,
    #[serde(serialize_with = "round2_opt")]
    invested: Option<f64>,
    #[serde(serialize_with = "round2_opt")]
    pnl: Option<f64>,
    #[serde(serialize_with = "round2_opt")]
    historic_return_percent: Option<f64>,
    #[serde(serialize_with = "round2_opt")]
    daily_return_percent: Option<f64>,
    #[serde(skip)]
    is_cash: bool,
}

impl PortfolioContext {
    pub fn from_portfolio(portfolio: &Portfolio, currency: &str, network_status: &str) -> Self {
        Self::from_portfolio_with_policy(portfolio, currency, network_status, None)
    }

    /// Like [`Self::from_portfolio`], but the single-position risk flag uses
    /// the policy's `single_position_limit_percent` (default 25 %), so it
    /// agrees with `review` and `check_policy`. Cash is never a "position"
    /// for that limit.
    pub fn from_portfolio_with_policy(
        portfolio: &Portfolio,
        currency: &str,
        network_status: &str,
        policy: Option<&Policy>,
    ) -> Self {
        let single_limit = policy
            .and_then(|p| p.constraints.single_position_limit_percent)
            .unwrap_or(SINGLE_POSITION_CONCENTRATION_THRESHOLD);
        let breakdown = portfolio.value_breakdown();
        let total = breakdown.total;
        let mut allocation: HashMap<String, (Option<Decimal>, usize)> = HashMap::new();
        let mut positions = Vec::new();
        let mut data_quality_flags = Vec::new();

        for position in &portfolio.positions {
            let priced = position.get_ticker().is_none() || position.has_market_price();
            let value_dec = priced.then(|| position.balance_dec());
            let asset_class = position.get_asset_class().to_string();
            let is_cash = position.is_cash();

            let allocation_entry = allocation
                .entry(asset_class.clone())
                .or_insert((Some(Decimal::ZERO), 0));
            allocation_entry.0 = match (allocation_entry.0, value_dec) {
                (Some(sum), Some(v)) => Some(sum + v),
                _ => None,
            };
            allocation_entry.1 += 1;

            if position.get_ticker().is_some()
                && position.price_currency().is_some()
                && !position.has_complete_cost_basis()
            {
                data_quality_flags.push(format!(
                    "Missing cost basis for {}; historic closes or FX rates for its purchases are unavailable (a recorded `Price` on each purchase avoids the lookup).",
                    position.get_name()
                ));
            }

            if position.get_ticker().is_none() && !is_cash {
                data_quality_flags.push(format!(
                    "No ticker for {}; live pricing and market movement are unavailable.",
                    position.get_name()
                ));
            }

            if position.get_ticker().is_some() && position.has_native_price() && position.is_stale()
            {
                data_quality_flags.push(format!(
                    "Price for {} is the last known value from the quote cache (as of {}).",
                    position.get_name(),
                    position.price_as_of().unwrap_or("unknown")
                ));
            }

            if position.price_status() == PriceStatus::FxUnavailable {
                data_quality_flags.push(format!(
                    "No FX rate from {} to the base currency for {}; position excluded from totals and invested unknown.",
                    position.price_currency().unwrap_or("?"),
                    position.get_name()
                ));
            } else if position.get_ticker().is_some() && !position.has_market_price() {
                data_quality_flags.push(format!(
                    "No current market price for {}; position excluded from totals.",
                    position.get_name()
                ));
            }

            positions.push(PositionContext {
                name: position.get_name().to_string(),
                ticker: position.get_ticker().map(ToString::to_string),
                asset_class,
                quantity: position.get_amount(),
                // Native quote even without FX; only `value` needs the rate.
                price: (position.get_ticker().is_some() && position.has_native_price())
                    .then(|| position.market_price()),
                // Unknown stays null: never label a foreign quote as base.
                price_currency: if position.get_ticker().is_some() {
                    position
                        .price_currency()
                        .map(str::to_uppercase)
                        .or_else(|| position.has_market_price().then(|| currency.to_uppercase()))
                } else {
                    None
                },
                price_as_of: (position.get_ticker().is_some() && position.has_native_price())
                    .then(|| position.price_as_of().map(String::from))
                    .flatten(),
                is_stale: position.get_ticker().is_some()
                    && position.has_native_price()
                    && position.is_stale(),
                // Same cents as MCP `valueBase` (`money`), so summary totals can
                // be the sum of the values this report actually shows.
                value: value_dec.map(cents),
                weight_percent: value_dec.map(|v| percent(v, total)),
                average_cost: position.average_cost(),
                invested: position.total_invested(),
                pnl: position.pnl(),
                historic_return_percent: position.historic_variation_percent(),
                daily_return_percent: position.native_daily_variation_percent(),
                is_cash,
            });
        }

        let mut allocation = allocation
            .into_iter()
            .map(|(asset_class, (value, position_count))| {
                // Same rounded figures the position rows report. A class with an
                // unpriced position keeps a null value (not a partial sum).
                let parts: Vec<f64> = positions
                    .iter()
                    .filter(|p| p.asset_class == asset_class)
                    .filter_map(|p| p.value)
                    .collect();
                AllocationContext {
                    asset_class,
                    value: value.map(|_| sum_money(parts)),
                    percent: value.map(|v| percent(v, total)),
                    position_count,
                }
            })
            .collect::<Vec<_>>();
        let key = |v: Option<f64>| v.unwrap_or(f64::NEG_INFINITY);
        // Value descending, unknown values last; ties by name so the order is
        // deterministic (the class map is a HashMap).
        allocation.sort_by(|a, b| {
            key(b.value)
                .partial_cmp(&key(a.value))
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.asset_class.cmp(&b.asset_class))
        });

        positions.sort_by(|a, b| {
            key(b.value)
                .partial_cmp(&key(a.value))
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });
        let complete = positions.iter().all(|p| p.value.is_some());
        let unpriced = positions.iter().filter(|p| p.value.is_none()).count();
        if !complete {
            // Every weight against a partial total is overstated; null them
            // all (positions, allocation, summary) and show the range-free
            // values only.
            for p in positions.iter_mut() {
                p.weight_percent = None;
            }
            for a in allocation.iter_mut() {
                a.percent = None;
            }
        }

        let largest = positions.iter().find(|p| p.value.is_some());
        let largest_position = largest.map(|p| p.name.clone());
        let largest_position_percent = largest.filter(|_| complete).and_then(|p| p.weight_percent);

        let summary = ContextSummary {
            incomplete: (!complete).then(|| incomplete_banner(unpriced)),
            data_status: if !complete {
                "partial"
            } else if positions.iter().any(|p| p.is_stale) {
                "stale"
            } else {
                "complete"
            },
            total_value: sum_money(positions.iter().filter_map(|p| p.value)),
            cash_value: sum_money(
                positions
                    .iter()
                    .filter(|p| p.is_cash)
                    .filter_map(|p| p.value),
            ),
            securities_value: sum_money(
                positions
                    .iter()
                    .filter(|p| !p.is_cash)
                    .filter_map(|p| p.value),
            ),
            cash_percent: complete.then(|| percent(breakdown.cash, total)),
            position_count: positions.len(),
            largest_position,
            largest_position_percent,
        };

        let risk_flags = risk_flags(&summary, &allocation, &positions, complete, single_limit);
        // One status for every surface, derived from the quotes actually
        // present (the loader's status is only a hint).
        let _ = network_status;
        let network_status = crate::mcp::analytics::network_status(portfolio);
        if portfolio
            .positions
            .iter()
            .any(|p| p.cost_basis_at_current_fx())
        {
            data_quality_flags.push(
                "Invested and PnL of foreign-currency positions convert purchase costs at the current FX rate, not the rate on the purchase date.".to_string(),
            );
        }
        if network_status != "connected" {
            data_quality_flags.push(format!(
                "Network status is {network_status}; quote-dependent values may be incomplete."
            ));
        }
        data_quality_flags.sort();
        data_quality_flags.dedup();

        Self {
            generated_at: Local::now().to_rfc3339(),
            currency: currency.to_string(),
            network_status: network_status.to_string(),
            summary,
            allocation,
            positions,
            risk_flags,
            data_quality_flags,
            complete,
            cash_buffer: CashBuffer::new(portfolio, policy),
            follow_up_commands: vec![
                "portfolio_rs balances [JSON_FILE]".to_string(),
                "portfolio_rs allocation [JSON_FILE]".to_string(),
                "portfolio_rs performance [JSON_FILE]".to_string(),
                "portfolio_rs context [JSON_FILE] --format json".to_string(),
            ],
        }
    }

    /// Replace the suggested follow-ups (MCP names its tools instead of CLI commands).
    pub fn set_follow_up_commands(&mut self, commands: Vec<String>) {
        self.follow_up_commands = commands;
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(Into::into)
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::new();

        out.push_str("# Portfolio Context\n\n");
        if let Some(banner) = &self.summary.incomplete {
            out.push_str(&format!("> **{banner}**\n\n"));
        }
        out.push_str(&format!("Generated: {}\n", self.generated_at));
        out.push_str(&format!("Currency: {}\n", self.currency));
        out.push_str(&format!("Network Status: {}\n", self.network_status));
        out.push_str(&format!("Data Status: {}\n", self.summary.data_status));
        out.push('\n');

        out.push_str("## Summary\n\n");
        out.push_str(&format!(
            "- Total Value: {}\n",
            money(self.summary.total_value, &self.currency)
        ));
        out.push_str(&format!(
            "- Cash: {} ({})\n",
            money(self.summary.cash_value, &self.currency),
            self.summary
                .cash_percent
                .map(pct)
                .unwrap_or_else(|| "n/a".to_string())
        ));
        if let (Some(min), Some(surplus)) = (self.cash_buffer.minimum, self.cash_buffer.surplus) {
            out.push_str(&format!(
                "- Cash Buffer: minimum {}, {} {}\n",
                money(min, &self.currency),
                if surplus < 0.0 {
                    "shortfall"
                } else {
                    "surplus"
                },
                money(surplus.abs(), &self.currency)
            ));
        }
        out.push_str(&format!(
            "- Securities: {}\n",
            money(self.summary.securities_value, &self.currency)
        ));
        out.push_str(&format!("- Positions: {}\n", self.summary.position_count));
        if let Some(largest_position) = &self.summary.largest_position {
            out.push_str(&format!(
                "- Largest Position: {} ({})\n",
                escape_markdown(largest_position),
                self.summary
                    .largest_position_percent
                    .map(pct)
                    .unwrap_or_else(|| "n/a".to_string())
            ));
        }

        out.push_str("\n## Allocation\n\n");
        out.push_str("| Asset Class | Value | Weight | Positions |\n");
        out.push_str("| --- | ---: | ---: | ---: |\n");
        for item in &self.allocation {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                escape_markdown(&item.asset_class),
                opt(item.value.map(|v| money(v, &self.currency))),
                opt(item.percent.map(pct)),
                item.position_count
            ));
        }

        out.push_str("\n## Positions\n\n");
        out.push_str("| Name | Ticker | Class | Quantity | Price | Value | Weight | PnL | Hist % | Day % |\n");
        out.push_str("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n");
        for position in &self.positions {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                escape_markdown(&position.name),
                position
                    .ticker
                    .as_deref()
                    .map(escape_markdown)
                    .unwrap_or_else(|| "-".to_string()),
                escape_markdown(&position.asset_class),
                quantity(position.quantity),
                opt(position.price.map(|value| {
                    let shown = money(
                        value,
                        position.price_currency.as_deref().unwrap_or(&self.currency),
                    );
                    if position.is_stale {
                        format!("{shown} (stale)")
                    } else {
                        shown
                    }
                })),
                opt(position.value.map(|v| money(v, &self.currency))),
                opt(position.weight_percent.map(pct)),
                position
                    .pnl
                    .map(|value| money(value, &self.currency))
                    .unwrap_or_else(|| "-".to_string()),
                position
                    .historic_return_percent
                    .map(pct)
                    .unwrap_or_else(|| "-".to_string()),
                position
                    .daily_return_percent
                    .map(pct)
                    .unwrap_or_else(|| "-".to_string())
            ));
        }

        out.push_str("\n## Risk Flags\n\n");
        push_list(&mut out, &self.risk_flags);

        out.push_str("\n## Data Quality Flags\n\n");
        push_list(&mut out, &self.data_quality_flags);

        out.push_str("\n## Useful Follow-Up Commands\n\n");
        for command in &self.follow_up_commands {
            out.push_str(&format!("- `{command}`\n"));
        }

        out
    }
}

fn risk_flags(
    summary: &ContextSummary,
    allocation: &[AllocationContext],
    positions: &[PositionContext],
    complete: bool,
    single_limit: f64,
) -> Vec<String> {
    let mut flags = Vec::new();

    if summary.position_count == 0 || (summary.total_value <= 0.0 && complete) {
        flags.push("Portfolio is empty or has no positive value.".to_string());
        return flags;
    }
    if !complete {
        // Weights against a partial total overstate every priced position
        // (e.g. cash shows as 100 % offline): no concentration or liquidity
        // flags until all positions are priced.
        flags.push(
            "Data is incomplete; concentration and liquidity flags are not evaluated.".to_string(),
        );
        return flags;
    }

    for position in positions.iter().filter(|p| !p.is_cash) {
        let Some(weight) = position.weight_percent else {
            continue;
        };
        if weight > single_limit {
            flags.push(format!(
                "Single-position concentration: {} is {} of the portfolio (limit {}, cash excluded).",
                position.name,
                pct(weight),
                pct(single_limit)
            ));
        }
    }

    for item in allocation {
        let Some(percent) = item.percent else {
            continue;
        };
        if percent >= ASSET_CLASS_CONCENTRATION_THRESHOLD {
            flags.push(format!(
                "Asset-class concentration: {} is {} of the portfolio.",
                item.asset_class,
                pct(percent)
            ));
        }
    }

    if let Some(cash) = summary.cash_percent.filter(|c| *c < LOW_CASH_THRESHOLD) {
        flags.push(format!(
            "Cash allocation is low at {}; consider checking liquidity needs against policy.",
            pct(cash)
        ));
    }

    if flags.is_empty() {
        flags.push("No simple concentration or liquidity flags triggered.".to_string());
    }

    flags
}

fn percent(value: Decimal, total: Decimal) -> f64 {
    percent_of(value, total).map(to_f64).unwrap_or(0.0)
}

fn money(value: f64, currency: &str) -> String {
    format!("{value:.2} {currency}")
}

fn opt(value: Option<String>) -> String {
    value.unwrap_or_else(|| "-".to_string())
}

fn pct(value: f64) -> String {
    format!("{value:.2}%")
}

fn quantity(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else if value.abs() >= 1.0 {
        format!("{value:.4}")
    } else {
        format!("{value:.8}")
    }
}

fn escape_markdown(value: &str) -> String {
    value.replace('|', "\\|")
}

fn push_list(out: &mut String, items: &[String]) {
    if items.is_empty() {
        out.push_str("- None\n");
        return;
    }

    for item in items {
        out.push_str(&format!("- {}\n", escape_markdown(item)));
    }
}

/// The incomplete banner shown first in the summary and in context.md.
fn incomplete_banner(unpriced: usize) -> String {
    format!(
        "INCOMPLETE: {unpriced} unpriced position(s). Totals cover priced positions only; weights and shares are not shown."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_output_format_parse() {
        assert_eq!(
            ContextOutputFormat::parse("markdown").unwrap(),
            ContextOutputFormat::Markdown
        );
        assert_eq!(
            ContextOutputFormat::parse("md").unwrap(),
            ContextOutputFormat::Markdown
        );
        assert_eq!(
            ContextOutputFormat::parse("json").unwrap(),
            ContextOutputFormat::Json
        );
        assert!(ContextOutputFormat::parse("table").is_err());
    }

    #[test]
    fn test_quantity_formatting() {
        assert_eq!(quantity(10.0), "10");
        assert_eq!(quantity(10.123456), "10.1235");
        assert_eq!(quantity(0.123456789), "0.12345679");
    }
}
