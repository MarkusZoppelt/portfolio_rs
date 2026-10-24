//! rmcp server handler: tool router and resources.

use super::analytics;
use super::dto::*;
use super::ServedPortfolio;
use crate::portfolio::Portfolio;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    service::RequestContext,
    tool, tool_handler, tool_router, ErrorData as McpError, Json, RoleServer, ServerHandler,
};
use serde_json::json;
use std::sync::Arc;

const MAX_LIMIT: u32 = 500;
const DEFAULT_LIMIT: u32 = 100;

#[derive(Clone)]
pub struct PortfolioServer {
    served: Arc<ServedPortfolio>,
    tool_router: ToolRouter<Self>,
}

fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

fn parse_day(field: &str, value: &Option<String>) -> Result<Option<String>, ToolError> {
    match value {
        None => Ok(None),
        Some(s) => super::snapshots::strict_date(s)
            .map(|d| Some(d.format("%Y-%m-%d").to_string()))
            .ok_or_else(|| {
                ToolError::invalid(format!("{field} must be a date in YYYY-MM-DD format"))
            }),
    }
}

impl PortfolioServer {
    pub fn new(served: ServedPortfolio) -> Self {
        Self {
            served: Arc::new(served),
            tool_router: Self::tool_router(),
        }
    }

    fn check_id(&self, id: &Option<String>) -> Result<(), ToolError> {
        match id {
            Some(id) if id != &self.served.id => Err(ToolError::new(
                "unknown_portfolio",
                format!("unknown portfolioId {id:?}; see list_portfolios"),
            )),
            _ => Ok(()),
        }
    }

    /// `invalid_argument` unless `class` is used by a position or the policy.
    async fn check_class(
        &self,
        portfolio: &Portfolio,
        class: &Option<String>,
    ) -> Result<(), ToolError> {
        let Some(class) = class else { return Ok(()) };
        let mut known: Vec<String> = portfolio
            .positions
            .iter()
            .map(|p| p.get_asset_class().to_string())
            .collect();
        if let Some(policy) = self.served.state.policy().await {
            known.extend(policy.allocations.iter().map(|a| a.asset_class.clone()));
        }
        if known.iter().any(|k| k.eq_ignore_ascii_case(class)) {
            return Ok(());
        }
        known.sort();
        known.dedup();
        Err(ToolError::invalid(format!(
            "unknown assetClass {class:?}; known: {}",
            known.join(", ")
        )))
    }

    async fn base_currency(&self) -> String {
        self.served.state.currency().await
    }

    async fn meta(&self, portfolio: &Portfolio, warnings: Vec<Finding>) -> Meta {
        let base_currency = self.base_currency().await;
        let data_quality = analytics::data_quality_summary(portfolio, &base_currency);
        let (price_as_of, stale_positions) = analytics::price_freshness(portfolio);
        let complete = data_quality.affected_positions.is_empty();
        Meta {
            data_status: analytics::data_status(complete, !stale_positions.is_empty()).to_string(),
            schema_version: SCHEMA_VERSION,
            portfolio_id: self.served.id.clone(),
            as_of: now_rfc3339(),
            base_currency,
            complete,
            data_quality,
            has_stale: !stale_positions.is_empty(),
            network_status: analytics::network_status(portfolio).to_string(),
            price_as_of,
            stale_positions,
            warnings,
        }
    }

    async fn portfolio(&self, refresh: bool) -> Portfolio {
        if refresh {
            self.served.state.refresh_prices().await;
        }
        self.served.state.portfolio().await
    }

    fn policy_options(&self) -> analytics::PolicyOptions {
        self.served.policy_options
    }

    fn policy_finding(&self) -> Option<Finding> {
        self.served.policy_error.as_ref().map(|_| {
            Finding::new(
                "invalid_policy",
                Severity::Critical,
                Category::Policy,
                "The policy file could not be loaded; see the server log.".to_string(),
            )
        })
    }

    /// The CLI context with follow-ups naming MCP tools instead of CLI commands.
    async fn context(&self) -> eyre::Result<crate::context::PortfolioContext> {
        let mut ctx = self.served.state.get_context().await?;
        ctx.set_follow_up_commands(vec![
            "get_positions".to_string(),
            "get_allocation".to_string(),
            "check_policy".to_string(),
            "get_transactions".to_string(),
            "get_context {\"format\": \"json\"}".to_string(),
        ]);
        Ok(ctx)
    }

    fn uri(&self, suffix: &str) -> String {
        format!("portfolio://{}/{suffix}", self.served.id)
    }
}

#[tool_router(router = tool_router)]
impl PortfolioServer {
    #[tool(
        name = "get_positions",
        description = "Holdings with quantity, price, base-currency value, weight, cost basis and PnL. Money is rounded to 2 decimals; null means unknown (e.g. no live quote).",
        annotations(title = "Get positions", read_only_hint = true, open_world_hint = true)
    )]
    async fn get_positions(
        &self,
        Parameters(input): Parameters<GetPositionsInput>,
    ) -> Result<Json<PositionsOutput>, ToolError> {
        self.check_id(&input.portfolio_id)?;
        let portfolio = self.portfolio(input.refresh.unwrap_or(false)).await;
        self.check_class(&portfolio, &input.asset_class).await?;
        let base = self.base_currency().await;
        let pnl = analytics::portfolio_pnl(&portfolio);
        let (positions, total, cash) = analytics::position_rows(
            &portfolio,
            &base,
            input.include_manual.unwrap_or(true),
            input.asset_class.as_deref(),
            input.sort_by.unwrap_or_default(),
        );
        Ok(Json(PositionsOutput {
            meta: self
                .meta(&portfolio, analytics::data_quality(&portfolio))
                .await,
            total_value: money(total),
            cash_value: money(cash),
            invested: pnl.invested.map(money),
            unrealized_pnl: pnl.unrealized_pnl.map(money),
            unrealized_pnl_percent: pnl.unrealized_pnl_percent.map(pct),
            day_pnl: pnl.day_pnl.map(money),
            pnl_partial: pnl.partial,
            positions,
        }))
    }

    #[tool(
        name = "get_allocation",
        description = "Allocation by asset class against the policy targets and tolerance bands, with drift and the amount to reach each target.",
        annotations(
            title = "Get allocation",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn get_allocation(
        &self,
        Parameters(input): Parameters<GetAllocationInput>,
    ) -> Result<Json<AllocationOutput>, ToolError> {
        self.check_id(&input.portfolio_id)?;
        let portfolio = self.portfolio(false).await;
        let policy = self.served.state.policy().await;
        let alloc = analytics::allocation_with(
            &portfolio,
            policy.as_ref(),
            input.include_cash.unwrap_or(true),
            input.include_manual.unwrap_or(true),
            self.policy_options().possible_breach_min_share,
        );
        let mut warnings = analytics::data_quality(&portfolio);
        if policy.is_none() {
            warnings.push(
                self.policy_finding()
                    .unwrap_or_else(analytics::no_policy_finding),
            );
        }
        Ok(Json(AllocationOutput {
            meta: self.meta(&portfolio, warnings).await,
            policy: policy.as_ref().map(|p| PolicyRef {
                name: p.name.clone(),
                source: self.uri("policy"),
            }),
            total_value: money(alloc.total),
            classes: alloc.classes,
            untargeted_classes: alloc.untargeted,
            missing_classes: alloc.missing,
            excluded_positions: alloc.excluded_positions,
            excluded_value_share: alloc.excluded_value_share,
            excluded_value_basis: alloc.excluded_value_basis.map(String::from),
        }))
    }

    #[tool(
        name = "check_policy",
        description = "Check the portfolio against the investment policy: allocation bands, position and asset-class limits, cash minimum, and data quality. status is ok, warnings or breach.",
        annotations(title = "Check policy", read_only_hint = true, open_world_hint = true)
    )]
    async fn check_policy(
        &self,
        Parameters(input): Parameters<CheckPolicyInput>,
    ) -> Result<Json<PolicyCheckOutput>, ToolError> {
        self.check_id(&input.portfolio_id)?;
        let min = input.min_severity.unwrap_or(Severity::Info);
        let portfolio = self.portfolio(false).await;
        let policy = self.served.state.policy().await;
        let (mut findings, checks) = match &policy {
            Some(p) => {
                let c = analytics::check_policy_with(&portfolio, p, self.policy_options());
                (c.findings, c.checks)
            }
            None => (
                vec![self
                    .policy_finding()
                    .unwrap_or_else(analytics::no_policy_finding)],
                Vec::new(),
            ),
        };
        // Finance A6: policy status, counts, breaches and `findings` come
        // from the policy findings only. Data-quality warnings are added to
        // the top-level `warnings` (deduplicated) but never change
        // `policyStatus` or `counts`; `dataStatus` reports them.
        let same = |a: &Finding, b: &Finding| {
            a.code == b.code
                && a.category == b.category
                && a.position_id == b.position_id
                && a.asset_class == b.asset_class
        };
        let mut policy_findings: Vec<Finding> = Vec::new();
        // Answer 1: checks_incomplete is data quality, not a policy finding.
        let check_quality: Vec<Finding> = findings
            .iter()
            .filter(|f| f.category == Category::DataQuality)
            .cloned()
            .collect();
        findings.retain(|f| f.category != Category::DataQuality);
        for f in findings.drain(..) {
            if f.severity >= min && !policy_findings.iter().any(|u| same(u, &f)) {
                policy_findings.push(f);
            }
        }
        let mut counts = SeverityCounts::default();
        for f in &policy_findings {
            match f.severity {
                Severity::Info => counts.info += 1,
                Severity::Warning => counts.warning += 1,
                Severity::Critical => counts.critical += 1,
            }
        }
        let breaches: Vec<Finding> = policy_findings
            .iter()
            .filter(|f| f.severity == Severity::Critical)
            .cloned()
            .collect();
        let mut warnings: Vec<Finding> = policy_findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .cloned()
            .collect();
        if min <= Severity::Warning {
            for q in check_quality
                .into_iter()
                .chain(analytics::data_quality(&portfolio))
                .filter(|q| q.severity == Severity::Warning)
            {
                if !warnings.iter().any(|u| same(u, &q)) {
                    warnings.push(q);
                }
            }
        }
        let findings = policy_findings;
        let (status, policy_status) = if counts.critical > 0 {
            ("breach", "breaches")
        } else if counts.warning > 0 {
            ("warnings", "warnings")
        } else {
            ("ok", "ok")
        };
        let cash_buffer = crate::context::CashBuffer::new(&portfolio, policy.as_ref());
        Ok(Json(PolicyCheckOutput {
            meta: self.meta(&portfolio, warnings).await,
            policy_name: policy.map(|p| p.name),
            status: status.to_string(),
            policy_status: policy_status.to_string(),
            breaches,
            findings,
            constraint_checks: checks,
            counts,
            cash_buffer,
        }))
    }

    #[tool(
        name = "get_transactions",
        description = "Buy and sell transactions synthesized from the recorded purchases, oldest first, with filters and cursor pagination. A missing purchase price is filled from the historic close (priceSource historic_close).",
        annotations(
            title = "Get transactions",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn get_transactions(
        &self,
        Parameters(input): Parameters<GetTransactionsInput>,
    ) -> Result<Json<TransactionsOutput>, ToolError> {
        self.check_id(&input.portfolio_id)?;
        let from = parse_day("from", &input.from)?;
        let to = parse_day("to", &input.to)?;
        if let (Some(f), Some(t)) = (&from, &to) {
            if f > t {
                return Err(ToolError::invalid("from must not be after to"));
            }
        }
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT);
        if limit == 0 || limit > MAX_LIMIT {
            return Err(ToolError::invalid(format!(
                "limit must be between 1 and {MAX_LIMIT}"
            )));
        }
        let offset = match &input.cursor {
            None => 0usize,
            Some(c) => c
                .strip_prefix("o:")
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| {
                    ToolError::invalid("cursor is not a value returned as nextCursor")
                })?,
        };
        let portfolio = self.portfolio(false).await;
        let base = self.base_currency().await;
        self.check_class(&portfolio, &input.asset_class).await?;
        let file_date = std::fs::metadata(&self.served.path)
            .and_then(|m| m.modified())
            .ok()
            .map(|t| {
                chrono::DateTime::<chrono::Local>::from(t)
                    .format("%Y-%m-%d")
                    .to_string()
            });
        let all = analytics::transactions(&portfolio, &base, file_date);
        if let Some(id) = &input.position_id {
            if !portfolio
                .positions
                .iter()
                .any(|p| p.id() == Some(id.as_str()))
            {
                return Err(ToolError::new(
                    "not_found",
                    format!("unknown positionId {id:?}; see get_positions"),
                ));
            }
        }
        let matching: Vec<Transaction> = all
            .into_iter()
            .filter(|t| {
                input
                    .position_id
                    .as_ref()
                    .is_none_or(|p| &t.position_id == p)
            })
            .filter(|t| {
                input.ticker.as_ref().is_none_or(|q| {
                    t.ticker
                        .as_ref()
                        .is_some_and(|tk| tk.eq_ignore_ascii_case(q))
                })
            })
            .filter(|_| input.isin.is_none())
            .filter(|t| {
                input
                    .asset_class
                    .as_ref()
                    .is_none_or(|c| t.asset_class.eq_ignore_ascii_case(c))
            })
            .filter(|t| {
                input
                    .types
                    .as_ref()
                    .is_none_or(|ty| ty.contains(&t.tx_type))
            })
            .filter(|t| {
                from.as_ref()
                    .is_none_or(|f| t.date.as_ref().is_some_and(|d| d >= f))
            })
            .filter(|t| {
                to.as_ref()
                    .is_none_or(|e| t.date.as_ref().is_some_and(|d| d <= e))
            })
            .collect();
        let sum = |ty: TxType| -> Option<f64> {
            let mut total = 0.0;
            for t in matching.iter().filter(|t| t.tx_type == ty) {
                total += t.amount_base?;
            }
            Some(money_f(total))
        };
        let totals = TransactionTotals {
            count: matching.len(),
            buy_amount_base: sum(TxType::Buy),
            sell_amount_base: sum(TxType::Sell),
            deposit_amount_base: sum(TxType::Deposit),
            withdrawal_amount_base: sum(TxType::Withdrawal),
        };
        let mut warnings = Vec::new();
        let fx_missing: Vec<&Transaction> = matching
            .iter()
            .filter(|t| t.amount.is_some() && t.amount_base.is_none())
            .collect();
        let fx_historic_missing = fx_missing.len();
        let mut fx_positions: Vec<String> =
            fx_missing.iter().map(|t| t.position_id.clone()).collect();
        fx_positions.dedup();
        if !fx_positions.is_empty() {
            warnings.push(
                Finding::new(
                    "fx_historic_unavailable",
                    Severity::Warning,
                    Category::DataQuality,
                    format!("{fx_historic_missing} transaction(s) are in a foreign currency without a historic FX rate; their amountBase is null and the base totals that include them are null."),
                )
                .detail(json!({ "positionIds": fx_positions, "fxHistoricMissing": fx_historic_missing })),
            );
        }
        let end = (offset + limit as usize).min(matching.len());
        let next_cursor = (end < matching.len()).then(|| format!("o:{end}"));
        let page: Vec<Transaction> = matching
            .into_iter()
            .skip(offset)
            .take(limit as usize)
            .collect();
        // `complete` describes current values only (Finance A3); missing
        // historic FX is reported via fxHistoricMissing and the warning.
        let meta = self.meta(&portfolio, warnings).await;
        Ok(Json(TransactionsOutput {
            meta,
            source: "synthesized_from_purchases".to_string(),
            transactions: page,
            totals,
            fx_historic_missing,
            next_cursor,
        }))
    }

    #[tool(
        name = "list_portfolios",
        description = "Portfolios this server can read, with their ids for portfolioId. Paths are omitted unless includePaths is true.",
        annotations(
            title = "List portfolios",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_portfolios(
        &self,
        Parameters(input): Parameters<ListPortfoliosInput>,
    ) -> Result<Json<PortfoliosOutput>, ToolError> {
        let s = &self.served;
        let has_policy = s.state.policy().await.is_some();
        let data_status = self
            .meta(&self.portfolio(false).await, Vec::new())
            .await
            .data_status;
        Ok(Json(PortfoliosOutput {
            schema_version: SCHEMA_VERSION,
            as_of: now_rfc3339(),
            portfolios: vec![PortfolioEntry {
                id: s.id.clone(),
                label: s.label.clone(),
                mode: s.mode.to_string(),
                encrypted: s.encrypted(),
                has_policy,
                is_default: true,
                quote_source: s.quote_source.to_string(),
                data_status,
                path: input.include_paths.unwrap_or(false).then(|| s.path.clone()),
            }],
        }))
    }

    #[tool(
        name = "get_performance",
        description = "Performance for a period (day, week, month, mtd, ytd, 1y, since_inception, custom), split into net contributions, market (price + FX) effect and manual revaluation, with a time-weighted return. Read-only: uses local snapshots written by `portfolio_rs snapshot`, ending at today's live valuation.",
        annotations(
            title = "Get performance",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn get_performance(
        &self,
        Parameters(input): Parameters<GetPerformanceInput>,
    ) -> Result<Json<PerformanceOutput>, ToolError> {
        use super::performance as perf;
        use super::snapshots;
        use chrono::{Datelike, Duration, NaiveDate};
        self.check_id(&input.portfolio_id)?;
        let today = chrono::Local::now().date_naive();
        let parse = |v: &Option<String>, name: &str| -> Result<Option<NaiveDate>, ToolError> {
            v.as_deref()
                .map(|d| {
                    snapshots::strict_date(d).ok_or_else(|| {
                        ToolError::new("invalid_argument", format!("{name} must be YYYY-MM-DD"))
                    })
                })
                .transpose()
        };
        let (cfrom, cto) = (parse(&input.from, "from")?, parse(&input.to, "to")?);
        if input.period != Period::Custom && (cfrom.is_some() || cto.is_some()) {
            return Err(ToolError::new(
                "invalid_argument",
                "from and to are only allowed with period custom",
            ));
        }
        let path = self.served.snapshots_path.clone();
        let (snaps, skipped) = path.as_deref().map(snapshots::read).unwrap_or_default();
        let (from, to) = match input.period {
            Period::Day => (today, today),
            Period::Week => (today - Duration::days(6), today),
            Period::Month => (today - Duration::days(29), today),
            Period::Mtd => (today.with_day(1).unwrap_or(today), today),
            Period::Ytd => (
                NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap_or(today),
                today,
            ),
            Period::OneYear => (today - Duration::days(364), today),
            Period::SinceInception => (
                snaps
                    .first()
                    .and_then(|s| NaiveDate::parse_from_str(&s.date, "%Y-%m-%d").ok())
                    .map(|d| d + Duration::days(1))
                    .unwrap_or(today),
                today,
            ),
            Period::Custom => match (cfrom, cto) {
                (Some(f), Some(t)) if f <= t && t <= today => (f, t),
                (Some(_), Some(_)) => {
                    return Err(ToolError::new(
                        "invalid_argument",
                        "from must be on or before to, and to not in the future",
                    ))
                }
                _ => {
                    return Err(ToolError::new(
                        "invalid_argument",
                        "period custom needs from and to",
                    ))
                }
            },
        };
        let (from_s, to_s) = (
            from.format("%Y-%m-%d").to_string(),
            to.format("%Y-%m-%d").to_string(),
        );
        let portfolio = self.portfolio(false).await;
        let base = self.base_currency().await;
        let mut warnings = analytics::data_quality(&portfolio);
        let mut meta = self.meta(&portfolio, Vec::new()).await;
        let end_is_live = to == today;
        let end = if end_is_live {
            Some(snapshots::from_portfolio(
                &portfolio,
                &base,
                &to_s,
                meta.has_stale,
            ))
        } else {
            snaps.iter().rev().find(|s| s.date <= to_s).cloned()
        };
        // B1: the start is the last snapshot before `from`, at most
        // `max_age` days before the day before `from`; a historical end must
        // lie inside the window.
        let max_age = i64::from(input.max_start_age_days.unwrap_or(match input.period {
            Period::Day | Period::Week => 3,
            Period::Month | Period::Mtd => 5,
            Period::Ytd | Period::OneYear => 10,
            Period::SinceInception => 0,
            Period::Custom => match (to - from).num_days() + 1 {
                0..=7 => 3,
                8..=31 => 5,
                _ => 10,
            },
        }));
        let start_gap = |s: &snapshots::Snapshot| {
            NaiveDate::parse_from_str(&s.date, "%Y-%m-%d")
                .map(|d| (from - d).num_days() - 1)
                .unwrap_or(i64::MAX)
        };
        let start = snaps
            .iter()
            .rev()
            .find(|s| s.date < from_s)
            .filter(|s| start_gap(s) <= max_age)
            .cloned();
        let end = end.filter(|e| end_is_live || e.date >= from_s);
        let snapshot_days = snaps
            .iter()
            .filter(|s| s.date >= from_s && s.date <= to_s)
            .count();
        let period_days = (to - from).num_days() + 1;
        let covered =
            snapshot_days + usize::from(end_is_live && !snaps.iter().any(|s| s.date == to_s));
        let coverage = DataCoverage {
            snapshot_days,
            missing_days: (period_days - covered as i64).max(0),
            reconstructed: false,
            start_point_date: start.as_ref().map(|s| s.date.clone()),
            end_point_date: end.as_ref().map(|s| s.date.clone()),
            end_is_live,
            skipped_lines: skipped,
        };
        let mut out = PerformanceOutput {
            meta: self.meta(&portfolio, Vec::new()).await,
            period: serde_json::to_value(input.period)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default(),
            from: from_s.clone(),
            to: to_s.clone(),
            start_value: None,
            end_value: None,
            change: None,
            net_contributions: None,
            market_effect: None,
            income_effect: None,
            manual_revaluation: None,
            fx_effect: None,
            twr_percent: None,
            mwr_percent: None,
            method: "none".to_string(),
            data_coverage: coverage,
            breakdown: Vec::new(),
        };
        let (Some(start), Some(end)) = (start, end) else {
            warnings.push(Finding::new(
                "insufficient_history",
                Severity::Warning,
                Category::DataQuality,
                format!(
                    "No usable snapshot for {from_s}..{to_s}: the start needs one at most {max_age} day(s) before {from_s} \
                     (maxStartAgeDays) and a past end one inside the window; run `portfolio_rs snapshot` daily to build history."
                ),
            ));
            meta.warnings = warnings;
            out.meta = meta;
            return Ok(Json(out));
        };
        // Data status over the live valuation and both end points.
        let complete = start.complete && end.complete;
        let has_stale =
            meta.has_stale || start.data_status == "stale" || end.data_status == "stale";
        meta.complete = meta.complete && complete;
        meta.has_stale = has_stale;
        meta.data_status = analytics::data_status(meta.complete, has_stale).to_string();
        let txs = analytics::transactions(&portfolio, &base, None);
        let gap = start_gap(&start);
        if gap > 0 {
            warnings.push(
                Finding::new(
                    "start_point_gap",
                    Severity::Info,
                    Category::DataQuality,
                    format!(
                        "The start snapshot is from {}, {gap} day(s) before the day before {from_s}: the figures span {} days, not {period_days}.",
                        start.date,
                        period_days + gap
                    ),
                )
                .detail(json!({ "startPointDate": start.date, "gapDays": gap })),
            );
        }
        let vanished = perf::vanished(&start, &end, &txs);
        if !vanished.is_empty() {
            meta.complete = false;
            meta.data_status = analytics::data_status(false, has_stale).to_string();
            warnings.push(
                Finding::new(
                    "vanished_position",
                    Severity::Warning,
                    Category::DataQuality,
                    format!(
                        "{} position(s) held on {} are gone without a sell entry; their value is booked as an unknown external flow, not a loss, and the figures are partial.",
                        vanished.len(),
                        start.date
                    ),
                )
                .detail(json!({ "positionIds": vanished })),
            );
        }
        let effects = if complete {
            perf::position_effects(&start, &end, &txs)
        } else {
            None
        };
        let Some(effects) = effects else {
            meta.complete = false;
            meta.data_status = analytics::data_status(false, has_stale).to_string();
            warnings.push(Finding::new(
                "performance_partial",
                Severity::Warning,
                Category::DataQuality,
                "A valuation at the period start or end is missing prices, FX rates or flow amounts; figures are null.".to_string(),
            ));
            meta.warnings = warnings;
            out.meta = meta;
            return Ok(Json(out));
        };
        let t = perf::total(&effects);
        out.start_value = Some(perf::r2(t.start_value));
        out.end_value = Some(perf::r2(t.end_value));
        out.change = Some(perf::r2(t.change()));
        out.net_contributions = Some(perf::r2(t.net_contributions));
        out.market_effect = Some(perf::r2(t.market_effect));
        out.income_effect = Some(0.0);
        out.manual_revaluation = Some(perf::r2(t.manual_revaluation));
        if skipped > 0 {
            warnings.push(
                Finding::new(
                    "snapshot_lines_skipped",
                    Severity::Warning,
                    Category::DataQuality,
                    format!(
                        "{skipped} snapshot line(s) were skipped (bad JSON or a date not in YYYY-MM-DD form); the start point ({}) may be older than intended.",
                        start.date
                    ),
                )
                .detail(json!({ "skippedLines": skipped, "startPointDate": start.date })),
            );
        }
        let mut points = vec![start.clone()];
        points.extend(
            snaps
                .iter()
                .filter(|s| s.date > start.date && s.date < end.date)
                .cloned(),
        );
        points.push(end.clone());
        let (twr, method) = if points.iter().all(|p| p.complete) {
            perf::twr(&points, &txs)
        } else {
            (None, "modified_dietz")
        };
        if twr.is_none() {
            warnings.push(Finding::new(
                "performance_twr_unavailable",
                Severity::Info,
                Category::DataQuality,
                "The time-weighted return could not be computed for this period.".to_string(),
            ));
        }
        if twr.is_some() && method == "modified_dietz" {
            warnings.push(
                Finding::new(
                    "performance_approximate",
                    Severity::Info,
                    Category::DataQuality,
                    "No daily snapshot chain covers the period, so the return is a Modified Dietz approximation, not a time-weighted return.".to_string(),
                )
                .detail(json!({ "method": method })),
            );
        }
        out.twr_percent = twr.map(|v| (v * 100.0).round() / 100.0);
        out.method = method.to_string();
        let row = |key: String, e: &perf::Effects| PerformanceBreakdown {
            key,
            start_value: perf::r2(e.start_value),
            end_value: perf::r2(e.end_value),
            change: perf::r2(e.change()),
            net_contributions: perf::r2(e.net_contributions),
            market_effect: perf::r2(e.market_effect),
            manual_revaluation: perf::r2(e.manual_revaluation),
        };
        out.breakdown = match input.group_by.unwrap_or_default() {
            GroupBy::None => Vec::new(),
            GroupBy::AssetClass => perf::group_by_class(&effects)
                .iter()
                .map(|(k, e)| row(k.clone(), e))
                .collect(),
            GroupBy::Position => effects
                .iter()
                .map(|(k, (_, e))| row(k.clone(), e))
                .collect(),
        };
        meta.warnings = warnings;
        out.meta = meta;
        Ok(Json(out))
    }

    #[tool(
        name = "get_context",
        description = "LLM-ready portfolio context: totals, allocation and positions, as markdown (default) or JSON.",
        annotations(title = "Get context", read_only_hint = true, open_world_hint = true)
    )]
    async fn get_context(
        &self,
        Parameters(input): Parameters<GetContextInput>,
    ) -> Result<Json<ContextOutput>, ToolError> {
        self.check_id(&input.portfolio_id)?;
        let format = input.format.unwrap_or_default();
        let context = self
            .context()
            .await
            .map_err(|_| ToolError::new("internal", "could not build the portfolio context"))?;
        let (markdown, value) = match format {
            ContextFormat::Markdown => (Some(context.to_markdown()), None),
            ContextFormat::Json => {
                let json = context
                    .to_json()
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .ok_or_else(|| ToolError::new("internal", "could not serialize the context"))?;
                (None, Some(json))
            }
        };
        let portfolio = self.portfolio(false).await;
        Ok(Json(ContextOutput {
            meta: self
                .meta(&portfolio, analytics::data_quality(&portfolio))
                .await,
            format: match format {
                ContextFormat::Markdown => "markdown",
                ContextFormat::Json => "json",
            }
            .to_string(),
            markdown,
            context: value,
        }))
    }
}

fn resource(uri: String, name: &str, title: &str, mime: &str) -> Resource {
    Resource::new(uri, name)
        .with_title(title)
        .with_mime_type(mime)
}

fn text(uri: &str, mime: &str, body: String) -> ReadResourceResponse {
    ReadResourceResult::new(vec![ResourceContents::text(body, uri).with_mime_type(mime)]).into()
}

fn findings_catalog() -> serde_json::Value {
    let codes: Vec<_> = FINDING_CODES
        .iter()
        .map(|(code, category, severity, description)| {
            json!({ "code": code, "category": category, "defaultSeverity": severity, "description": description })
        })
        .collect();
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "finding": schemars::schema_for!(Finding),
        "codes": codes,
    })
}

/// rmcp reports argument deserialization failures (wrong type, unknown
/// field) as plain text. Re-shape them like every other tool error.
fn structured_argument_error(response: CallToolResponse) -> CallToolResponse {
    const PREFIX: &str = "failed to deserialize parameters:";
    if let CallToolResponse::Complete(result) = &response {
        if result.is_error == Some(true) && result.content.len() == 1 {
            if let Some(text) = result.content[0].as_text() {
                if let Some(rest) = text.text.strip_prefix(PREFIX) {
                    let err = ToolError::invalid(format!("invalid arguments: {}", rest.trim()));
                    if let Ok(r) =
                        rmcp::handler::server::tool::IntoCallToolResult::into_call_tool_result(err)
                    {
                        return r;
                    }
                }
            }
        }
    }
    response
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for PortfolioServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new(
            "portfolio_rs",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(
            "Read-only access to a local portfolio. Start with list_portfolios, then \
             get_positions / get_allocation / check_policy. Money is in the base currency \
             rounded to 2 decimals; null means unknown.",
        )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        let response = self.tool_router.call(tcc).await?;
        Ok(structured_argument_error(response))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = self.tool_router.list_all();
        if !self.served.network {
            // No tool reaches the network without a live quote provider.
            for tool in &mut tools {
                if let Some(a) = tool.annotations.as_mut() {
                    a.open_world_hint = Some(false);
                }
            }
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        // Prompts are not advertised in the capabilities.
        Err(McpError::method_not_found::<ListPromptsRequestMethod>())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut items = vec![resource(
            self.uri("context.md"),
            "context.md",
            "Portfolio context (markdown)",
            "text/markdown",
        )];
        if self.served.state.policy().await.is_some() {
            items.push(resource(
                self.uri("policy"),
                "policy",
                "Investment policy",
                "application/json",
            ));
        }
        if self.served.policy_md_path().is_some() {
            items.push(resource(
                self.uri("policy.md"),
                "policy.md",
                "Investment policy narrative",
                "text/markdown",
            ));
        }
        items.push(resource(
            "portfolio://schema/positions".into(),
            "schema/positions",
            "JSON Schema of a get_positions row",
            "application/schema+json",
        ));
        items.push(resource(
            "portfolio://schema/findings".into(),
            "schema/findings",
            "Finding schema and code catalog",
            "application/json",
        ));
        Ok(ListResourcesResult::with_all_items(items))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        let t = |uri: &str, name: &str, title: &str, mime: &str| {
            ResourceTemplate::new(uri, name)
                .with_title(title)
                .with_mime_type(mime)
        };
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            t(
                "portfolio://{id}/policy",
                "policy",
                "Investment policy",
                "application/json",
            ),
            t(
                "portfolio://{id}/policy.md",
                "policy.md",
                "Investment policy narrative",
                "text/markdown",
            ),
            t(
                "portfolio://{id}/context.md",
                "context.md",
                "Portfolio context (markdown)",
                "text/markdown",
            ),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri.as_str();
        let not_found = || McpError::resource_not_found(format!("resource not found: {uri}"), None);
        match uri {
            "portfolio://schema/positions" => {
                let schema = serde_json::to_string_pretty(&schemars::schema_for!(PositionRow))
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                return Ok(text(uri, "application/schema+json", schema));
            }
            "portfolio://schema/findings" => {
                let body = serde_json::to_string_pretty(&findings_catalog())
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                return Ok(text(uri, "application/json", body));
            }
            _ => {}
        }
        let rest = uri.strip_prefix("portfolio://").ok_or_else(not_found)?;
        let (id, what) = rest.split_once('/').ok_or_else(not_found)?;
        if id != self.served.id {
            return Err(not_found());
        }
        match what {
            "context.md" => {
                let ctx = self.context().await.map_err(|_| {
                    McpError::internal_error("could not build the portfolio context", None)
                })?;
                Ok(text(uri, "text/markdown", ctx.to_markdown()))
            }
            "policy" => {
                let policy = self.served.state.policy().await.ok_or_else(not_found)?;
                let body = json!({
                    "schemaVersion": SCHEMA_VERSION,
                    "portfolioId": self.served.id,
                    "source": self.served.state.policy_path().await.map(|_| "workspace").unwrap_or("file"),
                    "policy": policy,
                });
                let body = serde_json::to_string_pretty(&body)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(text(uri, "application/json", body))
            }
            "policy.md" => {
                let path = self.served.policy_md_path().ok_or_else(not_found)?;
                let body = std::fs::read_to_string(path).map_err(|_| {
                    McpError::internal_error("could not read INVESTMENT_POLICY.md", None)
                })?;
                Ok(text(uri, "text/markdown", body))
            }
            _ => Err(not_found()),
        }
    }
}
