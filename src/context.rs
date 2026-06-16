use std::{cmp::Ordering, collections::HashMap};

use chrono::Local;
use eyre::{eyre, Result};
use serde::Serialize;

use crate::portfolio::Portfolio;

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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextSummary {
    total_value: f64,
    cash_value: f64,
    securities_value: f64,
    cash_percent: f64,
    position_count: usize,
    largest_position: Option<String>,
    largest_position_percent: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AllocationContext {
    asset_class: String,
    value: f64,
    percent: f64,
    position_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PositionContext {
    name: String,
    ticker: Option<String>,
    asset_class: String,
    quantity: f64,
    price: Option<f64>,
    value: f64,
    weight_percent: f64,
    average_cost: Option<f64>,
    invested: Option<f64>,
    pnl: Option<f64>,
    historic_return_percent: Option<f64>,
    daily_return_percent: Option<f64>,
}

impl PortfolioContext {
    pub fn from_portfolio(portfolio: &Portfolio, currency: &str, network_status: &str) -> Self {
        let total_value = portfolio.get_total_value();
        let mut cash_value = 0.0;
        let mut securities_value = 0.0;
        let mut allocation: HashMap<String, (f64, usize)> = HashMap::new();
        let mut positions = Vec::new();
        let mut data_quality_flags = Vec::new();

        for position in &portfolio.positions {
            let value = position.market_value();
            let asset_class = position.get_asset_class().to_string();
            let is_cash = position.get_ticker().is_none()
                && position.get_asset_class().eq_ignore_ascii_case("cash");

            if is_cash {
                cash_value += value;
            } else {
                securities_value += value;
            }

            let allocation_entry = allocation.entry(asset_class.clone()).or_insert((0.0, 0));
            allocation_entry.0 += value;
            allocation_entry.1 += 1;

            if position.get_ticker().is_some() && position.total_invested().is_none() {
                data_quality_flags.push(format!(
                    "Missing cost basis for {}; add Purchase Price entries to improve PnL analysis.",
                    position.get_name()
                ));
            }

            if position.get_ticker().is_none() && !is_cash {
                data_quality_flags.push(format!(
                    "No ticker for {}; live pricing and market movement are unavailable.",
                    position.get_name()
                ));
            }

            if position.get_ticker().is_some() && position.market_price() <= 0.0 {
                data_quality_flags.push(format!(
                    "No current market price for {}; quote lookup may have failed.",
                    position.get_name()
                ));
            }

            positions.push(PositionContext {
                name: position.get_name().to_string(),
                ticker: position.get_ticker().map(ToString::to_string),
                asset_class,
                quantity: position.get_amount(),
                price: position.get_ticker().map(|_| position.market_price()),
                value,
                weight_percent: percent(value, total_value),
                average_cost: position.average_cost(),
                invested: position.total_invested(),
                pnl: position.pnl(),
                historic_return_percent: position.historic_variation_percent(),
                daily_return_percent: position.daily_variation_percent(),
            });
        }

        let mut allocation = allocation
            .into_iter()
            .map(|(asset_class, (value, position_count))| AllocationContext {
                asset_class,
                value,
                percent: percent(value, total_value),
                position_count,
            })
            .collect::<Vec<_>>();
        allocation.sort_by(|a, b| b.value.partial_cmp(&a.value).unwrap_or(Ordering::Equal));

        positions.sort_by(|a, b| b.value.partial_cmp(&a.value).unwrap_or(Ordering::Equal));

        let largest_position = positions.first().map(|p| p.name.clone());
        let largest_position_percent = positions.first().map(|p| p.weight_percent);

        let summary = ContextSummary {
            total_value,
            cash_value,
            securities_value,
            cash_percent: percent(cash_value, total_value),
            position_count: positions.len(),
            largest_position,
            largest_position_percent,
        };

        let risk_flags = risk_flags(&summary, &allocation, &positions);

        if network_status != "Connected" {
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
            follow_up_commands: vec![
                "portfolio_rs balances [JSON_FILE]".to_string(),
                "portfolio_rs allocation [JSON_FILE]".to_string(),
                "portfolio_rs performance [JSON_FILE]".to_string(),
                "portfolio_rs context [JSON_FILE] --format json".to_string(),
            ],
        }
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(Into::into)
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::new();

        out.push_str("# Portfolio Context\n\n");
        out.push_str(&format!("Generated: {}\n", self.generated_at));
        out.push_str(&format!("Currency: {}\n", self.currency));
        out.push_str(&format!("Network Status: {}\n\n", self.network_status));

        out.push_str("## Summary\n\n");
        out.push_str(&format!(
            "- Total Value: {}\n",
            money(self.summary.total_value, &self.currency)
        ));
        out.push_str(&format!(
            "- Cash: {} ({})\n",
            money(self.summary.cash_value, &self.currency),
            pct(self.summary.cash_percent)
        ));
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
                money(item.value, &self.currency),
                pct(item.percent),
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
                position
                    .price
                    .map(|value| money(value, &self.currency))
                    .unwrap_or_else(|| "-".to_string()),
                money(position.value, &self.currency),
                pct(position.weight_percent),
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
) -> Vec<String> {
    let mut flags = Vec::new();

    if summary.position_count == 0 || summary.total_value <= 0.0 {
        flags.push("Portfolio is empty or has no positive value.".to_string());
        return flags;
    }

    for position in positions {
        if position.weight_percent >= SINGLE_POSITION_CONCENTRATION_THRESHOLD {
            flags.push(format!(
                "Single-position concentration: {} is {} of the portfolio.",
                position.name,
                pct(position.weight_percent)
            ));
        }
    }

    for item in allocation {
        if item.percent >= ASSET_CLASS_CONCENTRATION_THRESHOLD {
            flags.push(format!(
                "Asset-class concentration: {} is {} of the portfolio.",
                item.asset_class,
                pct(item.percent)
            ));
        }
    }

    if summary.cash_percent < LOW_CASH_THRESHOLD {
        flags.push(format!(
            "Cash allocation is low at {}; consider checking liquidity needs against policy.",
            pct(summary.cash_percent)
        ));
    }

    if flags.is_empty() {
        flags.push("No simple concentration or liquidity flags triggered.".to_string());
    }

    flags
}

fn percent(value: f64, total: f64) -> f64 {
    if total > 0.0 {
        value / total * 100.0
    } else {
        0.0
    }
}

fn money(value: f64, currency: &str) -> String {
    format!("{value:.2} {currency}")
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
