//! Wire types for the MCP server (camelCase JSON, `schemaVersion: 1`).
//!
//! Money is a JSON number rounded to 2 decimals, quantities to 8 decimals,
//! and `null` always means "unknown", never zero.

use crate::money::{to_f64, Decimal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SCHEMA_VERSION: u32 = 1;

/// Round a money amount to 2 decimals for the wire.
pub fn money(d: Decimal) -> f64 {
    to_f64(d.round_dp(2))
}

/// Round a percentage (or percentage points) to 2 decimals.
pub fn pct(d: Decimal) -> f64 {
    to_f64(d.round_dp(2))
}

/// Round a quantity to 8 decimals.
pub fn qty(d: Decimal) -> f64 {
    to_f64(d.round_dp(8))
}

/// Round an `f64` money value to 2 decimals.
pub fn money_f(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Common fields of every tool response.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub schema_version: u32,
    pub portfolio_id: String,
    /// RFC 3339 timestamp with UTC offset.
    pub as_of: String,
    pub base_currency: String,
    /// False when any price or FX rate is missing: totals, weights and
    /// allocation then cover only the priced positions (see `dataQuality`).
    pub complete: bool,
    pub data_quality: DataQuality,
    /// True when any used price or FX rate came from the last-known-price
    /// cache (see `stalePositions`). Cached prices within the max age count
    /// towards `complete`.
    pub has_stale: bool,
    /// connected | partial | offline, derived from the quotes actually
    /// present; identical across all tools and `get_context`.
    pub network_status: String,
    /// Oldest `priceAsOf` among the priced market positions; null when none
    /// reports one.
    pub price_as_of: Option<String>,
    /// `pos_` ids whose price or FX rate came from the cache.
    pub stale_positions: Vec<String>,
    /// Data-quality and policy warnings, each emitted once per response.
    pub warnings: Vec<Finding>,
}

/// What is missing when `complete` is false.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataQuality {
    /// Tickers without a live quote.
    pub missing_prices: Vec<String>,
    /// FX pairs (`FROM/TO`) without a rate, e.g. `USD/EUR`.
    pub missing_fx: Vec<String>,
    /// `pos_` ids whose value is unknown.
    pub affected_positions: Vec<String>,
    /// Asset classes whose value is unknown or incomplete.
    pub affected_classes: Vec<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Allocation,
    Constraint,
    DataQuality,
    Policy,
}

/// A machine-readable finding. Messages never contain monetary amounts.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub code: String,
    pub severity: Severity,
    pub category: Category,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

impl Finding {
    pub fn new(code: &str, severity: Severity, category: Category, message: String) -> Self {
        Self {
            code: code.to_string(),
            severity,
            category,
            message,
            position_id: None,
            asset_class: None,
            detail: None,
        }
    }

    pub fn position(mut self, id: Option<&str>) -> Self {
        self.position_id = id.map(String::from);
        self
    }

    pub fn class(mut self, class: &str) -> Self {
        self.asset_class = Some(class.to_string());
        self
    }

    pub fn detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

/// Every finding code the server can emit (`portfolio://schema/findings`).
pub const FINDING_CODES: &[(&str, &str, &str, &str)] = &[
    (
        "allocation_out_of_band",
        "allocation",
        "warning",
        "Asset class outside its target band (critical beyond twice the tolerance).",
    ),
    (
        "unknown_asset_class",
        "allocation",
        "warning",
        "Holdings in an asset class the policy has no target for.",
    ),
    (
        "missing_target_class",
        "allocation",
        "warning",
        "Policy targets an asset class with no holdings.",
    ),
    (
        "single_position_limit",
        "constraint",
        "critical",
        "A position exceeds single_position_limit_percent.",
    ),
    (
        "asset_class_limit",
        "constraint",
        "critical",
        "An asset class exceeds asset_class_limit_percent.",
    ),
    (
        "cash_below_minimum_amount",
        "constraint",
        "critical",
        "Cash is below minimum_cash_amount.",
    ),
    (
        "cash_months_unavailable",
        "constraint",
        "info",
        "minimum_cash_months is set but monthly expenses are unknown.",
    ),
    (
        "checks_incomplete",
        "policy",
        "warning",
        "Prices or FX rates are missing; allocation and limits are judged on the priced part and only certain breaches are reported.",
    ),
    (
        "possible_breach",
        "allocation|constraint",
        "warning",
        "With missing prices a band or limit might be violated; checked on a partial basis, not a confirmed breach.",
    ),
    (
        "no_live_price",
        "data_quality",
        "warning",
        "No live quote; the position is excluded from totals.",
    ),
    (
        "fx_unavailable",
        "data_quality",
        "warning",
        "No FX rate to the base currency; the position is excluded from totals.",
    ),
    (
        "missing_cost_basis",
        "data_quality",
        "info",
        "Purchases are missing or have no price; PnL is unknown.",
    ),
    (
        "missing_ticker",
        "data_quality",
        "info",
        "Non-cash position without a ticker (manual valuation).",
    ),
    (
        "duplicate_ticker",
        "data_quality",
        "warning",
        "The same ticker appears in more than one position.",
    ),
    (
        "cost_basis_current_fx",
        "data_quality",
        "info",
        "Invested and PnL of foreign-currency positions use the current FX rate, not the rate on the purchase date.",
    ),
    (
        "fx_historic_unavailable",
        "data_quality",
        "info",
        "Transaction amounts in a foreign currency have no historic FX rate; amountBase is null.",
    ),
    (
        "no_policy",
        "policy",
        "warning",
        "No investment policy is loaded.",
    ),
    (
        "invalid_policy",
        "policy",
        "critical",
        "The policy file could not be loaded or failed validation.",
    ),
];

/// A structured tool error (`isError: true`). Never carries monetary values.
#[derive(Debug, Clone, Serialize)]
pub struct ToolError {
    pub code: &'static str,
    pub message: String,
}

impl ToolError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid_argument", message)
    }
}

impl rmcp::handler::server::tool::IntoCallToolResult for ToolError {
    fn into_call_tool_result(self) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
        let body = serde_json::json!({ "error": { "code": self.code, "message": self.message } });
        Ok(
            rmcp::model::CallToolResult::error(vec![rmcp::model::ContentBlock::text(
                body.to_string(),
            )])
            .into(),
        )
    }
}

// ---------------------------------------------------------------- inputs

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortBy {
    #[default]
    Value,
    Name,
    Pnl,
    DayChange,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetPositionsInput {
    /// Portfolio id from list_portfolios (defaults to the served portfolio).
    pub portfolio_id: Option<String>,
    /// Only positions of this asset class (case-insensitive).
    pub asset_class: Option<String>,
    /// Include manually valued positions (no ticker, not cash). Default true.
    pub include_manual: Option<bool>,
    /// Re-fetch quotes instead of using the 60 s cache. Default false.
    pub refresh: Option<bool>,
    /// Sort order. Default value (descending).
    pub sort_by: Option<SortBy>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetAllocationInput {
    pub portfolio_id: Option<String>,
    /// Include cash in the allocation base. Default true.
    pub include_cash: Option<bool>,
    /// Include manually valued positions. Default true.
    pub include_manual: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckPolicyInput {
    pub portfolio_id: Option<String>,
    /// Drop findings below this severity. Default info.
    pub min_severity: Option<Severity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TxType {
    Buy,
    Sell,
    /// Cash paid in. A cash position without recorded entries appears as
    /// one opening deposit of its balance (`synthesized: true`), dated at the
    /// earliest purchase date, else the data file's modification date.
    Deposit,
    /// Cash paid out (negative cash entry).
    Withdrawal,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetTransactionsInput {
    pub portfolio_id: Option<String>,
    /// Inclusive start date, YYYY-MM-DD.
    pub from: Option<String>,
    /// Inclusive end date, YYYY-MM-DD.
    pub to: Option<String>,
    pub position_id: Option<String>,
    /// Case-insensitive ticker filter.
    pub ticker: Option<String>,
    /// ISIN filter. Positions carry no ISIN yet, so a set value matches nothing.
    pub isin: Option<String>,
    pub asset_class: Option<String>,
    pub types: Option<Vec<TxType>>,
    /// Page size, 1..=500. Default 100.
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<u32>,
    /// Opaque cursor from a previous nextCursor.
    pub cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListPortfoliosInput {
    /// Include local file paths. Default false.
    pub include_paths: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContextFormat {
    #[default]
    Markdown,
    Json,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetContextInput {
    pub portfolio_id: Option<String>,
    /// markdown (default) or json.
    pub format: Option<ContextFormat>,
}

// --------------------------------------------------------------- outputs

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PositionRow {
    /// Stable `pos_…` id.
    pub id: String,
    pub name: String,
    pub ticker: Option<String>,
    /// Not tracked yet; always null.
    pub isin: Option<String>,
    pub asset_class: String,
    /// cash | market | manual
    pub kind: String,
    pub quantity: f64,
    /// Latest price in priceCurrency; null when unknown.
    pub price: Option<f64>,
    pub price_currency: Option<String>,
    /// When the provider set `price` (RFC 3339); null when not reported.
    pub price_as_of: Option<String>,
    /// Currency the holding is denominated in: the quote currency for market
    /// positions (null when unknown), the base currency for cash and manual.
    pub currency: Option<String>,
    /// True when the price or its FX rate came from the last-known-price cache.
    pub is_stale: bool,
    /// live | cache | fixture | none
    pub price_source: String,
    /// Base-currency units per unit of priceCurrency; null when no conversion applies.
    pub fx_rate: Option<f64>,
    /// live | stale (from the cache) | fx_unavailable (native price known,
    /// no FX rate) | unavailable | manual | cash
    pub price_status: String,
    /// Value in the base currency; null when the price or FX rate is unavailable.
    pub value_base: Option<f64>,
    pub weight_percent: Option<f64>,
    pub average_cost: Option<f64>,
    pub invested: Option<f64>,
    pub unrealized_pnl: Option<f64>,
    pub unrealized_pnl_percent: Option<f64>,
    pub day_change_percent: Option<f64>,
    pub purchases_count: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PositionsOutput {
    #[serde(flatten)]
    pub meta: Meta,
    pub total_value: f64,
    pub cash_value: f64,
    /// Portfolio-level cost basis of the priced market positions (base
    /// currency); positions without the input are left out (`pnlPartial`).
    pub invested: Option<f64>,
    /// Unrealized PnL of the priced market positions.
    pub unrealized_pnl: Option<f64>,
    pub unrealized_pnl_percent: Option<f64>,
    /// Today's PnL of the priced market positions vs. the previous close.
    pub day_pnl: Option<f64>,
    /// True when invested/PnL/dayPnl leave out a market position (no price,
    /// FX, cost basis or previous close).
    pub pnl_partial: bool,
    pub positions: Vec<PositionRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRef {
    pub name: String,
    /// Resource URI of the policy.
    pub source: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClassAllocation {
    pub asset_class: String,
    /// Null when a position in the class has no price or FX rate.
    pub value: Option<f64>,
    /// Share of the total; null when the class value or the total is unknown
    /// (any included position unpriced).
    pub actual_percent: Option<f64>,
    /// With unpriced positions: the lowest and highest possible share,
    /// whatever the excluded positions are worth (0 up to their maximum
    /// plausible value, see README). Absent when the total is complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_percent_min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_percent_max: Option<f64>,
    pub target_percent: Option<f64>,
    pub tolerance_percent: Option<f64>,
    pub band_low: Option<f64>,
    pub band_high: Option<f64>,
    /// actualPercent - targetPercent, in percentage points.
    pub drift_points: Option<f64>,
    /// within_band | below_band | above_band | untargeted | unknown. With
    /// unpriced positions a band status is only set when it holds for the
    /// whole possible range; otherwise unknown.
    pub status: String,
    /// Base-currency amount to buy (positive) or sell (negative) to reach the
    /// target; null when the class value or the total is unknown.
    pub amount_to_target: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AllocationOutput {
    #[serde(flatten)]
    pub meta: Meta,
    pub policy: Option<PolicyRef>,
    pub total_value: f64,
    pub classes: Vec<ClassAllocation>,
    pub untargeted_classes: Vec<String>,
    pub missing_classes: Vec<String>,
    /// Ids of positions without a base value; drift and percentages cover the
    /// priced part only.
    pub excluded_positions: Vec<String>,
    /// Excluded positions' estimated share (percent of priced total plus
    /// their estimate); null when one of them has no estimate.
    pub excluded_value_share: Option<f64>,
    /// last_price (last cached price, any age) | cost_basis (at least one
    /// position fell back to cost basis); null without a share.
    pub excluded_value_basis: Option<String>,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConstraintCheckRow {
    pub code: String,
    pub limit_percent: Option<f64>,
    /// Null when judged on a partial total; see the min/max range then.
    pub actual_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_percent_min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_percent_max: Option<f64>,
    pub limit_amount: Option<f64>,
    pub actual_amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_class: Option<String>,
    pub passed: bool,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct SeverityCounts {
    pub info: usize,
    pub warning: usize,
    pub critical: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PolicyCheckOutput {
    #[serde(flatten)]
    pub meta: Meta,
    pub policy_name: Option<String>,
    /// ok | warnings | breach
    pub status: String,
    /// Critical findings only (hard limits exceeded, allocation far outside
    /// its band, invalid policy), deduplicated. Warning-severity findings are
    /// in the top-level `warnings`, next to the data-quality warnings.
    pub breaches: Vec<Finding>,
    /// Every policy finding at or above `minSeverity`.
    pub findings: Vec<Finding>,
    pub constraint_checks: Vec<ConstraintCheckRow>,
    pub counts: SeverityCounts,
    /// Cash vs. the policy's minimum cash amount (base currency).
    pub cash_buffer: crate::context::CashBuffer,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    /// `tx_<positionId>_<index>`
    pub id: String,
    /// YYYY-MM-DD, null when the purchase has no date.
    pub date: Option<String>,
    /// True for an opening deposit synthesized from a cash balance without
    /// entries (its date is inferred, not recorded).
    pub synthesized: bool,
    #[serde(rename = "type")]
    pub tx_type: TxType,
    pub position_id: String,
    pub ticker: Option<String>,
    pub asset_class: String,
    /// Absolute quantity.
    pub quantity: f64,
    pub price: Option<f64>,
    /// recorded | historic_close | missing | cash (deposits)
    pub price_source: String,
    /// Recorded fees; 0 when the purchase records none (the same 0 that
    /// `amount` uses), also for a synthesized opening deposit.
    pub fees: Option<f64>,
    /// quantity * price (+ fees for buys, - fees for sells) in `currency`;
    /// null when the price is missing.
    pub amount: Option<f64>,
    pub currency: Option<String>,
    /// `amount` in the base currency; null when currency differs (no historic FX).
    pub amount_base: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransactionTotals {
    pub count: usize,
    /// Null when any matching buy has an unknown base amount.
    pub buy_amount_base: Option<f64>,
    pub sell_amount_base: Option<f64>,
    pub deposit_amount_base: Option<f64>,
    pub withdrawal_amount_base: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransactionsOutput {
    #[serde(flatten)]
    pub meta: Meta,
    /// Always synthesized_from_purchases until a ledger exists.
    pub source: String,
    pub transactions: Vec<Transaction>,
    /// Totals over all matching transactions (not just this page).
    pub totals: TransactionTotals,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioEntry {
    pub id: String,
    pub label: String,
    /// simple | workspace
    pub mode: String,
    pub encrypted: bool,
    pub has_policy: bool,
    pub is_default: bool,
    /// Where quotes come from: yahoo | offline | fixture.
    pub quote_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortfoliosOutput {
    pub schema_version: u32,
    pub as_of: String,
    pub portfolios: Vec<PortfolioEntry>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextOutput {
    #[serde(flatten)]
    pub meta: Meta,
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
}
