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
        Some(s) => chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(|d| Some(d.format("%Y-%m-%d").to_string()))
            .map_err(|_| {
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
        Meta {
            schema_version: SCHEMA_VERSION,
            portfolio_id: self.served.id.clone(),
            as_of: now_rfc3339(),
            base_currency,
            complete: data_quality.affected_positions.is_empty(),
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
        let alloc = analytics::allocation(
            &portfolio,
            policy.as_ref(),
            input.include_cash.unwrap_or(true),
            input.include_manual.unwrap_or(true),
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
                let c = analytics::check_policy(&portfolio, p);
                (c.findings, c.checks)
            }
            None => (
                vec![self
                    .policy_finding()
                    .unwrap_or_else(analytics::no_policy_finding)],
                Vec::new(),
            ),
        };
        // Data-quality warnings live once, in the top-level warnings.
        findings.retain(|f| f.severity >= min);
        let mut counts = SeverityCounts::default();
        for f in &findings {
            match f.severity {
                Severity::Info => counts.info += 1,
                Severity::Warning => counts.warning += 1,
                Severity::Critical => counts.critical += 1,
            }
        }
        // Breaches: critical findings only, one per (code, position, class).
        let mut breaches: Vec<Finding> = Vec::new();
        for f in findings.iter().filter(|f| f.severity == Severity::Critical) {
            let dup = breaches.iter().any(|b| {
                b.code == f.code && b.position_id == f.position_id && b.asset_class == f.asset_class
            });
            if !dup {
                breaches.push(f.clone());
            }
        }
        let status = if !breaches.is_empty() {
            "breach"
        } else if findings.iter().any(|f| f.severity >= Severity::Warning) {
            "warnings"
        } else {
            "ok"
        };
        // Top-level warnings: data quality plus warning-severity policy findings.
        let mut warnings = analytics::data_quality(&portfolio);
        for f in findings.iter().filter(|f| f.severity == Severity::Warning) {
            if !warnings.iter().any(|w| {
                w.code == f.code && w.position_id == f.position_id && w.asset_class == f.asset_class
            }) {
                warnings.push(f.clone());
            }
        }
        let cash_buffer = crate::context::CashBuffer::new(&portfolio, policy.as_ref());
        Ok(Json(PolicyCheckOutput {
            meta: self.meta(&portfolio, warnings).await,
            policy_name: policy.map(|p| p.name),
            status: status.to_string(),
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
        let all = analytics::transactions(&portfolio, &base);
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
        if matching
            .iter()
            .any(|t| t.amount.is_some() && t.amount_base.is_none())
        {
            warnings.push(Finding::new(
                "fx_historic_unavailable",
                Severity::Info,
                Category::DataQuality,
                "Some transactions are in a foreign currency; amountBase is null without historic FX rates.".to_string(),
            ));
        }
        let end = (offset + limit as usize).min(matching.len());
        let next_cursor = (end < matching.len()).then(|| format!("o:{end}"));
        let page: Vec<Transaction> = matching
            .into_iter()
            .skip(offset)
            .take(limit as usize)
            .collect();
        Ok(Json(TransactionsOutput {
            meta: self.meta(&portfolio, warnings).await,
            source: "synthesized_from_purchases".to_string(),
            transactions: page,
            totals,
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
                path: input.include_paths.unwrap_or(false).then(|| s.path.clone()),
            }],
        }))
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
