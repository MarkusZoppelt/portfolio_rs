//! Shared application state and serializable DTOs.
//!
//! [`AppState`] is the embedding-friendly facade over the portfolio domain:
//! the HTTP API server and external GUIs (e.g. a Tauri app) drive the
//! application exclusively through it. All DTOs serialize as `camelCase`
//! JSON so they can be consumed directly by web frontends.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eyre::{bail, eyre, Result, WrapErr};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::money::{dec, percent_change, to_f64, Decimal};
use crate::services::atomic_write::write_atomic_with_backup;
use crate::{
    context::PortfolioContext,
    create_live_portfolio_in,
    decision::generate_decision_draft,
    doctor::{check_workspace, WorkspaceHealth},
    document::write_dated_document,
    load_portfolio_file,
    policy::{AllocationTarget, Constraints, Policy, RiskProfile},
    positions_to_string,
    report::generate_markdown_report,
    review::Review,
    simulate::{simulate_rebalance, RebalanceSimulation},
    validate::{validate_portfolio_file, ValidationReport},
    AppConfig, NetworkStatus, Portfolio, PortfolioPosition, Purchase,
};

/// How long a fetched portfolio (with live quotes) stays fresh before the
/// next [`AppState::portfolio`] call re-fetches quotes.
const PORTFOLIO_CACHE_TTL: Duration = Duration::from_secs(60);

struct CachedPortfolio {
    portfolio: Portfolio,
    fetched_at: Instant,
}

/// Shared mutable state wrapped for async access.
pub struct AppState {
    pub positions_json: RwLock<String>,
    pub file_path: RwLock<Option<String>>,
    pub workspace_dir: RwLock<Option<String>>,
    pub policy: RwLock<Option<Policy>>,
    pub currency: RwLock<String>,
    pub network_status: RwLock<NetworkStatus>,
    portfolio_cache: RwLock<Option<CachedPortfolio>>,
}

fn path_to_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(String::from)
        .ok_or_else(|| eyre!("path is not valid UTF-8: {}", path.display()))
}

impl AppState {
    pub fn new(currency: String) -> Self {
        Self {
            positions_json: RwLock::new(String::from("[]")),
            file_path: RwLock::new(None),
            workspace_dir: RwLock::new(None),
            policy: RwLock::new(None),
            currency: RwLock::new(currency),
            network_status: RwLock::new(NetworkStatus::Disconnected),
            portfolio_cache: RwLock::new(None),
        }
    }

    /// Load a portfolio file (plaintext or `.gpg`) into state.
    pub async fn load_file(&self, path: &str) -> Result<()> {
        let data = load_portfolio_file(path)
            .wrap_err_with(|| format!("failed to load portfolio file: {}", path))?;
        {
            let mut file_path = self.file_path.write().await;
            *file_path = Some(path.to_string());
        }
        {
            let mut workspace_dir = self.workspace_dir.write().await;
            *workspace_dir = None;
        }
        self.set_positions_json(data).await;
        Ok(())
    }

    /// Load a workspace directory, reading `positions.json` and `portfolio/policy.toml`.
    pub async fn load_workspace(&self, dir: &str, cfg: &mut AppConfig) -> Result<()> {
        let positions_path = PathBuf::from(dir).join("positions.json");
        if !positions_path.exists() {
            bail!(
                "workspace does not contain positions.json: {}",
                positions_path.display()
            );
        }

        self.load_file(&path_to_string(&positions_path)?).await?;

        {
            let mut workspace_dir = self.workspace_dir.write().await;
            *workspace_dir = Some(dir.to_string());
        }

        let policy_path = PathBuf::from(dir).join("portfolio/policy.toml");
        if policy_path.exists() {
            let policy = Policy::from_file(&path_to_string(&policy_path)?)
                .wrap_err_with(|| format!("failed to load policy: {}", policy_path.display()))?;
            let mut p = self.policy.write().await;
            *p = Some(policy);
        }

        cfg.set_workspace(dir);
        cfg.save()?;
        Ok(())
    }

    /// Remember a standalone positions file and clear any workspace.
    pub async fn load_simple_file(&self, path: &str, cfg: &mut AppConfig) -> Result<()> {
        self.load_file(path).await?;
        cfg.set_portfolio_file(path);
        cfg.save()?;
        Ok(())
    }

    /// Load or reload policy from the default workspace location or an explicit path.
    pub async fn load_policy(&self, explicit_path: Option<String>) -> Result<()> {
        let path = if let Some(p) = explicit_path {
            p
        } else if let Some(dir) = self.workspace_dir.read().await.clone() {
            path_to_string(&PathBuf::from(dir).join("portfolio/policy.toml"))?
        } else {
            bail!("no workspace loaded and no policy path provided");
        };

        let policy = Policy::from_file(&path)
            .wrap_err_with(|| format!("failed to load policy: {}", path))?;
        let mut p = self.policy.write().await;
        *p = Some(policy);
        Ok(())
    }

    /// Set the active policy directly from a TOML string.
    pub async fn set_policy_from_toml(&self, toml: &str) -> Result<()> {
        let policy = toml
            .parse::<Policy>()
            .wrap_err("failed to parse policy TOML")?;
        let mut p = self.policy.write().await;
        *p = Some(policy);
        Ok(())
    }

    pub async fn workspace_dir(&self) -> Option<String> {
        self.workspace_dir.read().await.clone()
    }

    pub async fn policy(&self) -> Option<Policy> {
        self.policy.read().await.clone()
    }

    /// Resolve the effective policy file path.
    pub async fn policy_path(&self) -> Option<String> {
        if let Some(dir) = self.workspace_dir.read().await.clone() {
            let path = PathBuf::from(dir).join("portfolio/policy.toml");
            return path.to_str().map(String::from);
        }
        None
    }

    /// Replace the raw JSON and refresh live prices.
    pub async fn set_positions_json(&self, json: String) {
        {
            let mut positions_json = self.positions_json.write().await;
            *positions_json = json;
        }
        self.refresh_prices().await;
    }

    /// Re-fetch live prices for the currently loaded positions, bypassing
    /// the portfolio cache.
    pub async fn refresh_prices(&self) {
        let portfolio = self.fetch_portfolio().await;
        // Cache the enriched JSON (with names filled in from Yahoo) for saving.
        if let Ok(enriched) = positions_to_string(&portfolio.positions) {
            let mut positions_json = self.positions_json.write().await;
            *positions_json = enriched;
        }
    }

    /// Build a live portfolio from the current JSON.
    ///
    /// Results are cached for a short TTL so bursts of reads (e.g. several
    /// HTTP requests) do not hammer the quote provider. Use
    /// [`AppState::refresh_prices`] to force a re-fetch.
    pub async fn portfolio(&self) -> Portfolio {
        {
            let cache = self.portfolio_cache.read().await;
            if let Some(cached) = cache.as_ref() {
                if cached.fetched_at.elapsed() < PORTFOLIO_CACHE_TTL {
                    return cached.portfolio.clone();
                }
            }
        }
        self.fetch_portfolio().await
    }

    /// Fetch live quotes, update the network status, and refill the cache.
    async fn fetch_portfolio(&self) -> Portfolio {
        let json = self.positions_json.read().await.clone();
        let currency = self.currency().await;
        let (portfolio, status) = create_live_portfolio_in(json, &currency, false).await;
        *self.network_status.write().await = status;
        *self.portfolio_cache.write().await = Some(CachedPortfolio {
            portfolio: portfolio.clone(),
            fetched_at: Instant::now(),
        });
        portfolio
    }

    /// Drop the cached portfolio so the next read re-fetches quotes.
    async fn invalidate_portfolio_cache(&self) {
        *self.portfolio_cache.write().await = None;
    }

    pub async fn file_path(&self) -> Option<String> {
        self.file_path.read().await.clone()
    }

    pub async fn currency(&self) -> String {
        self.currency.read().await.clone()
    }

    pub async fn set_currency(&self, currency: String) {
        {
            let mut cur = self.currency.write().await;
            *cur = currency;
        }
        // Cached totals were converted to the old base currency.
        self.invalidate_portfolio_cache().await;
    }

    /// Persist the current positions back to the original file.
    ///
    /// Refuses to write `.gpg`-backed portfolios: decrypted content must
    /// never be written to disk. The write is atomic and keeps a single
    /// `.bak` beside the file.
    pub async fn save(&self) -> Result<()> {
        let path = self
            .file_path
            .read()
            .await
            .clone()
            .ok_or_else(|| eyre!("no portfolio file is loaded"))?;
        if path.ends_with(".gpg") {
            bail!(
                "refusing to overwrite encrypted portfolio file {}: \
                 writing decrypted data to disk is not supported",
                path
            );
        }
        let json = {
            let mut guard = self.positions_json.write().await;
            // Give legacy positions stable ids on first write.
            if let Ok(mut positions) = serde_json::from_str::<Vec<PortfolioPosition>>(&guard) {
                if crate::position::ensure_position_ids(&mut positions) {
                    *guard = positions_to_string(&positions)?;
                }
            }
            guard.clone()
        };
        // Atomic replace + one backup: a crash must not leave the portfolio
        // file truncated or empty.
        write_atomic_with_backup(Path::new(&path), json.as_bytes())
            .wrap_err_with(|| format!("failed to write portfolio file: {}", path))?;
        Ok(())
    }

    /// Whether the loaded portfolio can be persisted with [`AppState::save`].
    pub async fn can_save(&self) -> bool {
        matches!(
            self.file_path.read().await.as_deref(),
            Some(path) if !path.ends_with(".gpg")
        )
    }

    pub async fn get_positions(&self) -> Vec<PositionDto> {
        let portfolio = self.portfolio().await;
        portfolio
            .positions
            .iter()
            .enumerate()
            .map(|(i, p)| position_to_dto(i, p))
            .collect()
    }

    pub async fn get_position(&self, id: usize) -> Result<PositionDto> {
        let portfolio = self.portfolio().await;
        portfolio
            .positions
            .get(id)
            .map(|p| position_to_dto(id, p))
            .ok_or_else(|| eyre!("position not found"))
    }

    pub async fn get_portfolio_summary(&self) -> PortfolioSummaryDto {
        let portfolio = self.portfolio().await;
        let breakdown = portfolio.value_breakdown();
        let allocation = portfolio.get_allocation();

        // Summed over every position with a cost basis (unlike
        // `cost_basis_totals`, which skips cash and unpriced holdings).
        let mut total_invested = Decimal::ZERO;
        let mut total_pnl = Decimal::ZERO;
        let positions: Vec<PositionDto> = portfolio
            .positions
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if let Some(inv) = p.total_invested_dec() {
                    total_invested += inv;
                }
                if let Some(pnl) = p.pnl_dec() {
                    total_pnl += pnl;
                }
                position_to_dto(i, p)
            })
            .collect();

        let pnl_percent = percent_change(breakdown.total, total_invested)
            .map(to_f64)
            .unwrap_or(0.0);

        PortfolioSummaryDto {
            total_value: to_f64(breakdown.total),
            total_invested: to_f64(total_invested),
            total_pnl: to_f64(total_pnl),
            pnl_percent,
            day_change: breakdown.day_pnl(),
            day_change_percent: breakdown.day_percent(),
            cash_value: to_f64(breakdown.cash),
            securities_value: to_f64(breakdown.securities),
            allocation,
            positions,
        }
    }

    pub async fn get_allocation(&self) -> Vec<AllocationItemDto> {
        let portfolio = self.portfolio().await;
        let total = portfolio.total_value_dec();
        let mut items: Vec<AllocationItemDto> = portfolio
            .get_allocation()
            .into_iter()
            .map(|(asset_class, percent)| AllocationItemDto {
                asset_class,
                percent,
                value: to_f64(total * dec(percent) / Decimal::ONE_HUNDRED),
            })
            .collect();
        items.sort_by(|a, b| b.percent.partial_cmp(&a.percent).unwrap_or(Ordering::Equal));
        items
    }

    pub async fn get_performance(&self) -> PerformanceDto {
        let portfolio = self.portfolio().await;
        let breakdown = portfolio.value_breakdown();
        let current_value = to_f64(breakdown.total);
        let cash_value = to_f64(breakdown.cash);
        let securities_value = to_f64(breakdown.securities);

        // Cash is neither a gain nor a loss: do not subtract invested from
        // total value (which includes cash).
        let (total_invested, unrealized_pnl) = portfolio.cost_basis_totals();
        let pnl_percent = portfolio.hist_percent();

        let day_pnl = breakdown.day_pnl();
        let day_percent = breakdown.day_percent();
        let movers = portfolio.day_movers();

        let to_dto = |(name, percent, pnl): (String, f64, f64)| TopMoverDto { name, percent, pnl };
        let (gainers, losers) = crate::portfolio::top_movers(movers, 3);
        let gainers: Vec<TopMoverDto> = gainers.into_iter().map(to_dto).collect();
        let losers: Vec<TopMoverDto> = losers.into_iter().map(to_dto).collect();

        PerformanceDto {
            total_value: current_value,
            securities_value,
            cash_value,
            invested: total_invested,
            unrealized_pnl,
            pnl_percent,
            day_pnl,
            day_percent,
            top_gainers: gainers,
            top_losers: losers,
        }
    }

    pub async fn add_position(&self, req: CreatePositionRequest) -> Result<PositionDto> {
        let position = PortfolioPosition {
            id: Some(crate::position::new_position_id()),
            name: req.name,
            ticker: req.ticker,
            asset_class: req.asset_class,
            amount: req.amount,
            last_spot: Decimal::ZERO,
            price_status: crate::position::PriceStatus::Live,
            price_currency: None,
            fx_rate: None,
            purchases: req
                .purchases
                .into_iter()
                .map(|p| Purchase {
                    date: p.date,
                    quantity: p.quantity,
                    price: p.price,
                    fees: p.fees,
                })
                .collect(),
            previous_close: None,
        };

        let idx;
        let dto;
        {
            let mut json = self.positions_json.write().await;
            let mut positions: Vec<PortfolioPosition> = serde_json::from_str(&json)
                .wrap_err("failed to parse current positions for update")?;
            positions.push(position);
            crate::position::ensure_position_ids(&mut positions);
            *json = positions_to_string(&positions)?;
            idx = positions.len() - 1;
            dto = position_to_dto(idx, &positions[idx]);
        }
        self.invalidate_portfolio_cache().await;
        Ok(dto)
    }

    pub async fn update_position(
        &self,
        id: usize,
        req: UpdatePositionRequest,
    ) -> Result<PositionDto> {
        let dto;
        {
            let mut json = self.positions_json.write().await;
            let mut positions: Vec<PortfolioPosition> = serde_json::from_str(&json)
                .wrap_err("failed to parse current positions for update")?;

            if id >= positions.len() {
                bail!("position not found");
            }

            let position = &mut positions[id];
            if let Some(name) = req.name {
                position.name = Some(name);
            }
            if let Some(ticker) = req.ticker {
                position.ticker = Some(ticker);
            }
            if let Some(asset_class) = req.asset_class {
                position.asset_class = asset_class;
            }
            if let Some(amount) = req.amount {
                position.set_amount(amount);
            }
            if let Some(purchases) = req.purchases {
                position.purchases = purchases
                    .into_iter()
                    .map(|p| Purchase {
                        date: p.date,
                        quantity: p.quantity,
                        price: p.price,
                        fees: p.fees,
                    })
                    .collect();
            }

            crate::position::ensure_position_ids(&mut positions);
            *json = positions_to_string(&positions)?;
            dto = position_to_dto(id, &positions[id]);
        }
        self.invalidate_portfolio_cache().await;
        Ok(dto)
    }

    pub async fn delete_position(&self, id: usize) -> Result<()> {
        {
            let mut json = self.positions_json.write().await;
            let mut positions: Vec<PortfolioPosition> = serde_json::from_str(&json)
                .wrap_err("failed to parse current positions for update")?;

            if id >= positions.len() {
                bail!("position not found");
            }

            positions.remove(id);
            crate::position::ensure_position_ids(&mut positions);
            *json = positions_to_string(&positions)?;
        }
        self.invalidate_portfolio_cache().await;
        Ok(())
    }

    /// Build a portfolio context briefing from the currently loaded portfolio.
    pub async fn get_context(&self) -> Result<PortfolioContext> {
        let portfolio = self.portfolio().await;
        let currency = self.currency.read().await.clone();
        let network_status = format!("{:?}", *self.network_status.read().await);
        Ok(PortfolioContext::from_portfolio(
            &portfolio,
            &currency,
            &network_status,
        ))
    }

    /// Review the current portfolio against the loaded policy.
    pub async fn get_review(&self) -> Result<Review> {
        let portfolio = self.portfolio().await;
        let currency = self.currency.read().await.clone();
        let policy = self
            .policy
            .read()
            .await
            .clone()
            .ok_or_else(|| eyre!("no investment policy is loaded"))?;
        Ok(Review::from_portfolio_and_policy(
            &portfolio, &policy, &currency,
        ))
    }

    /// Simulate rebalancing scenarios against the loaded policy.
    pub async fn get_simulation(&self) -> Result<RebalanceSimulation> {
        let portfolio = self.portfolio().await;
        let currency = self.currency.read().await.clone();
        let policy = self
            .policy
            .read()
            .await
            .clone()
            .ok_or_else(|| eyre!("no investment policy is loaded"))?;
        Ok(simulate_rebalance(&portfolio, &policy, &currency))
    }

    /// Run a workspace health check.
    pub async fn run_doctor(&self, dir: Option<String>) -> Result<WorkspaceHealth> {
        let dir = match dir {
            Some(d) => d,
            None => self
                .workspace_dir
                .read()
                .await
                .clone()
                .ok_or_else(|| eyre!("no workspace directory provided or loaded"))?,
        };
        check_workspace(&dir)
    }

    /// Validate the currently loaded portfolio file.
    pub async fn validate_portfolio(&self) -> Result<ValidationReport> {
        let file_path = self
            .file_path
            .read()
            .await
            .clone()
            .ok_or_else(|| eyre!("no portfolio file is loaded"))?;
        validate_portfolio_file(&file_path)
    }

    /// Generate a weekly Markdown report.
    pub async fn generate_report(
        &self,
        dir: Option<String>,
        dry_run: bool,
    ) -> Result<GeneratedDocument> {
        let dir = match dir {
            Some(d) => d,
            None => self
                .workspace_dir
                .read()
                .await
                .clone()
                .ok_or_else(|| eyre!("no workspace directory provided or loaded"))?,
        };
        let file_path = PathBuf::from(&dir).join(format!(
            "portfolio/reports/{}-weekly.md",
            chrono::Local::now().format("%Y-%m-%d")
        ));
        let file_path_str = path_to_string(&file_path)?;

        let filename = self.file_path.read().await.clone().unwrap_or_default();
        let policy_file = self
            .policy_path()
            .await
            .unwrap_or_else(|| "portfolio/policy.toml".to_string());
        let currency = self.currency.read().await.clone();

        let content = generate_markdown_report(
            &filename,
            &policy_file,
            &chrono::Local::now().format("%Y-%m-%d").to_string(),
            &currency,
        )
        .await;

        write_dated_document(
            &file_path,
            &content,
            dry_run,
            "weekly report",
            "Remove it to regenerate.",
        )?;

        Ok(GeneratedDocument {
            file_path: file_path_str,
            content,
            dry_run,
        })
    }

    /// Draft a structured decision record.
    pub async fn draft_decision(
        &self,
        title: Option<String>,
        dir: Option<String>,
        dry_run: bool,
    ) -> Result<GeneratedDocument> {
        let dir = match dir {
            Some(d) => d,
            None => self
                .workspace_dir
                .read()
                .await
                .clone()
                .ok_or_else(|| eyre!("no workspace directory provided or loaded"))?,
        };
        let title = title.unwrap_or_else(|| "Portfolio Rebalance Review".to_string());
        let slug = title
            .to_lowercase()
            .replace(' ', "-")
            .replace(|c: char| !c.is_alphanumeric() && c != '-', "");
        let file_path = PathBuf::from(&dir).join(format!(
            "portfolio/decisions/{}-{}.md",
            chrono::Local::now().format("%Y-%m-%d"),
            slug
        ));
        let file_path_str = path_to_string(&file_path)?;

        let filename = self.file_path.read().await.clone().unwrap_or_default();
        let policy_file = self
            .policy_path()
            .await
            .unwrap_or_else(|| "portfolio/policy.toml".to_string());
        let currency = self.currency.read().await.clone();

        let content = generate_decision_draft(&filename, &policy_file, &title, &currency).await;

        write_dated_document(
            &file_path,
            &content,
            dry_run,
            "decision record",
            "Use a different title.",
        )?;

        Ok(GeneratedDocument {
            file_path: file_path_str,
            content,
            dry_run,
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PositionDto {
    pub id: usize,
    pub name: Option<String>,
    pub ticker: Option<String>,
    pub asset_class: String,
    pub amount: f64,
    pub price: f64,
    pub value: f64,
    pub pnl: Option<f64>,
    pub pnl_percent: Option<f64>,
    pub day_change_percent: Option<f64>,
    pub average_cost: Option<f64>,
    pub invested: Option<f64>,
    pub purchases: Vec<PurchaseDto>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseDto {
    pub date: Option<String>,
    pub quantity: f64,
    pub price: Option<f64>,
    pub fees: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioSummaryDto {
    pub total_value: f64,
    pub total_invested: f64,
    pub total_pnl: f64,
    pub pnl_percent: f64,
    pub day_change: f64,
    pub day_change_percent: f64,
    pub cash_value: f64,
    pub securities_value: f64,
    pub allocation: HashMap<String, f64>,
    pub positions: Vec<PositionDto>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AllocationItemDto {
    pub asset_class: String,
    pub percent: f64,
    pub value: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceDto {
    pub total_value: f64,
    pub securities_value: f64,
    pub cash_value: f64,
    pub invested: f64,
    pub unrealized_pnl: f64,
    pub pnl_percent: f64,
    pub day_pnl: f64,
    pub day_percent: f64,
    pub top_gainers: Vec<TopMoverDto>,
    pub top_losers: Vec<TopMoverDto>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TopMoverDto {
    pub name: String,
    pub percent: f64,
    pub pnl: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CreatePositionRequest {
    pub name: Option<String>,
    pub ticker: Option<String>,
    pub asset_class: String,
    pub amount: f64,
    pub purchases: Vec<CreatePurchaseRequest>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CreatePurchaseRequest {
    pub date: Option<String>,
    pub quantity: f64,
    pub price: Option<f64>,
    pub fees: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePositionRequest {
    pub name: Option<String>,
    pub ticker: Option<String>,
    pub asset_class: Option<String>,
    pub amount: Option<f64>,
    pub purchases: Option<Vec<CreatePurchaseRequest>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedDocument {
    pub file_path: String,
    pub content: String,
    pub dry_run: bool,
}

/// Serializable view of [`AppConfig`] for GUI/API consumers.
///
/// The LLM API key is a write-only secret and is never included in this view.
/// See [`AppConfigDto::llm_api_key`].
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigDto {
    pub version: u32,
    pub currency: String,
    pub theme: crate::ThemeMode,
    pub last_mode: crate::AppMode,
    pub portfolio_file: Option<String>,
    pub workspace_dir: Option<String>,
    pub llm_provider_url: Option<String>,
    /// Write-only LLM API key. Never returned: [`From<&AppConfig>`] always
    /// sets this to `None`, so serialization emits `"llmApiKey": null` and
    /// the secret never appears in API or GUI payloads. Use
    /// [`Self::has_llm_api_key`] to tell whether a non-empty key is stored.
    ///
    /// On an incoming update, `null` or a missing `llmApiKey` must not clear
    /// a stored key — only an explicit non-empty string replaces it. Apply
    /// updates with [`Self::apply_to`], which implements that rule. The
    /// explicit clear is [`AppConfig::set_llm_api_key`] with an empty or
    /// whitespace-only string (stores `None`); `apply_to` forwards such a
    /// string to that setter. This crate has no config-update HTTP endpoint.
    #[serde(default)]
    pub llm_api_key: Option<String>,
    /// Whether a non-empty LLM API key is configured (`hasLlmApiKey`).
    ///
    /// Additive field. `#[serde(default)]` so JSON written before this field
    /// existed still deserializes; a missing value becomes `false`.
    #[serde(default)]
    pub has_llm_api_key: bool,
    pub llm_model: Option<String>,
}

impl AppConfigDto {
    /// Copy this view onto an existing [`AppConfig`].
    ///
    /// Every field except the API key is replaced. `llm_api_key` of `None`
    /// (JSON `null`, or a missing field) leaves the stored key unchanged.
    /// `Some(key)` is forwarded to [`AppConfig::set_llm_api_key`]: a non-empty
    /// string replaces the stored key; an empty or whitespace-only string
    /// clears it.
    pub fn apply_to(&self, cfg: &mut AppConfig) {
        cfg.version = self.version;
        cfg.currency = self.currency.clone();
        cfg.theme = self.theme;
        cfg.last_mode = self.last_mode;
        cfg.portfolio_file = self.portfolio_file.clone();
        cfg.workspace_dir = self.workspace_dir.clone();
        cfg.llm_provider_url = self.llm_provider_url.clone();
        cfg.llm_model = self.llm_model.clone();
        if let Some(key) = &self.llm_api_key {
            cfg.set_llm_api_key(key.clone());
        }
    }
}

fn llm_api_key_configured(key: &Option<String>) -> bool {
    key.as_ref().is_some_and(|value| !value.trim().is_empty())
}

impl From<&AppConfig> for AppConfigDto {
    fn from(cfg: &AppConfig) -> Self {
        Self {
            version: cfg.version,
            currency: cfg.currency.clone(),
            theme: cfg.theme,
            last_mode: cfg.last_mode,
            portfolio_file: cfg.portfolio_file.clone(),
            workspace_dir: cfg.workspace_dir.clone(),
            llm_provider_url: cfg.llm_provider_url.clone(),
            llm_api_key: None,
            has_llm_api_key: llm_api_key_configured(&cfg.llm_api_key),
            llm_model: cfg.llm_model.clone(),
        }
    }
}

/// Serializable view of [`Policy`] for GUI/API consumers.
///
/// Unlike [`Policy`] itself (which serializes as snake_case TOML), this DTO
/// serializes as camelCase JSON.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PolicyDto {
    pub version: String,
    pub name: String,
    pub base_currency: String,
    pub time_horizon_years: u32,
    pub risk_profile: RiskProfile,
    pub constraints: PolicyConstraintsDto,
    pub allocations: Vec<AllocationTargetDto>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PolicyConstraintsDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_cash_months: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_cash_amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub single_position_limit_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_class_limit_percent: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AllocationTargetDto {
    pub asset_class: String,
    pub target_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tolerance_percent: Option<f64>,
}

impl From<Constraints> for PolicyConstraintsDto {
    fn from(c: Constraints) -> Self {
        Self {
            minimum_cash_months: c.minimum_cash_months,
            minimum_cash_amount: c.minimum_cash_amount,
            single_position_limit_percent: c.single_position_limit_percent,
            asset_class_limit_percent: c.asset_class_limit_percent,
        }
    }
}

impl From<AllocationTarget> for AllocationTargetDto {
    fn from(a: AllocationTarget) -> Self {
        Self {
            asset_class: a.asset_class,
            target_percent: a.target_percent,
            tolerance_percent: a.tolerance_percent,
        }
    }
}

impl From<Policy> for PolicyDto {
    fn from(policy: Policy) -> Self {
        Self {
            version: policy.version,
            name: policy.name,
            base_currency: policy.base_currency,
            time_horizon_years: policy.time_horizon_years,
            risk_profile: policy.risk_profile,
            constraints: policy.constraints.into(),
            allocations: policy.allocations.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<&Policy> for PolicyDto {
    fn from(policy: &Policy) -> Self {
        policy.clone().into()
    }
}

fn position_to_dto(id: usize, position: &PortfolioPosition) -> PositionDto {
    PositionDto {
        id,
        name: position.get_name_option().map(String::from),
        ticker: position.get_ticker().map(String::from),
        asset_class: position.get_asset_class().to_string(),
        amount: position.get_amount(),
        price: position.market_price(),
        value: position.market_value(),
        pnl: position.pnl(),
        pnl_percent: position.historic_variation_percent(),
        day_change_percent: position.daily_variation_percent(),
        average_cost: position.average_cost(),
        invested: position.total_invested(),
        purchases: position
            .get_purchases()
            .iter()
            .map(|p| PurchaseDto {
                date: p.date.clone(),
                quantity: p.quantity,
                price: p.price,
                fees: p.fees,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::default_balanced_growth_policy;

    #[test]
    fn test_app_config_dto_serializes_camel_case() {
        let cfg = AppConfig::default();
        let dto = AppConfigDto::from(&cfg);
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["currency"], "EUR");
        assert_eq!(json["theme"], "dark");
        assert_eq!(json["lastMode"], "simple");
        assert!(json.get("portfolioFile").is_some());
        assert!(json.get("workspaceDir").is_some());
        assert!(json.get("llmProviderUrl").is_some());
        assert!(json.get("llmApiKey").is_some());
        assert!(json.get("llmModel").is_some());
        assert_eq!(json["hasLlmApiKey"], false);
    }

    #[test]
    fn test_app_config_dto_redacts_llm_api_key() {
        let mut cfg = AppConfig::default();
        cfg.set_llm_api_key("sk-test-FAKE-123");
        let dto = AppConfigDto::from(&cfg);
        let json = serde_json::to_string(&dto).unwrap();
        assert!(
            json.contains("\"llmApiKey\":null"),
            "expected redacted key, got {json}"
        );
        assert!(
            json.contains("\"hasLlmApiKey\":true"),
            "expected hasLlmApiKey, got {json}"
        );
        assert!(
            !json.contains("sk-test-FAKE-123"),
            "secret leaked into JSON: {json}"
        );
        assert!(dto.llm_api_key.is_none());
        assert!(dto.has_llm_api_key);

        let blank = AppConfig {
            llm_api_key: Some("   ".to_string()),
            ..Default::default()
        };
        assert!(!AppConfigDto::from(&blank).has_llm_api_key);
    }

    #[test]
    fn test_app_config_dto_old_json_without_has_llm_api_key_deserializes() {
        let dto = AppConfigDto::from(&AppConfig::default());
        let mut json = serde_json::to_value(&dto).unwrap();
        json.as_object_mut().unwrap().remove("hasLlmApiKey");
        let parsed: AppConfigDto = serde_json::from_value(json).unwrap();
        assert!(!parsed.has_llm_api_key);
        assert!(parsed.llm_api_key.is_none());
    }

    #[test]
    fn test_app_config_dto_apply_does_not_wipe_key_on_null() {
        let mut cfg = AppConfig::default();
        cfg.set_llm_api_key("sk-test-FAKE-123");
        cfg.set_currency("USD");

        let mut incoming = AppConfigDto::from(&cfg);
        incoming.currency = "EUR".to_string();
        assert!(incoming.llm_api_key.is_none());
        incoming.apply_to(&mut cfg);
        assert_eq!(cfg.llm_api_key.as_deref(), Some("sk-test-FAKE-123"));
        assert_eq!(cfg.currency, "EUR");

        // Missing llmApiKey deserializes as None and must not clear either.
        let mut json = serde_json::to_value(&incoming).unwrap();
        json.as_object_mut().unwrap().remove("llmApiKey");
        let missing: AppConfigDto = serde_json::from_value(json).unwrap();
        assert!(missing.llm_api_key.is_none());
        missing.apply_to(&mut cfg);
        assert_eq!(cfg.llm_api_key.as_deref(), Some("sk-test-FAKE-123"));

        incoming.llm_api_key = Some("sk-replaced".to_string());
        incoming.apply_to(&mut cfg);
        assert_eq!(cfg.llm_api_key.as_deref(), Some("sk-replaced"));

        // Explicit clear: empty / whitespace, same as AppConfig::set_llm_api_key.
        incoming.llm_api_key = Some("   ".to_string());
        incoming.apply_to(&mut cfg);
        assert!(cfg.llm_api_key.is_none());
    }

    #[test]
    fn test_policy_dto_serializes_camel_case() {
        let dto = PolicyDto::from(default_balanced_growth_policy());
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["riskProfile"], "moderate");
        assert!(json.get("baseCurrency").is_some());
        assert!(json.get("timeHorizonYears").is_some());
        assert!(json["constraints"].get("minimumCashMonths").is_some());
        let alloc = &json["allocations"][0];
        assert!(alloc.get("assetClass").is_some());
        assert!(alloc.get("targetPercent").is_some());
    }

    #[tokio::test]
    async fn test_save_refuses_gpg_files() {
        let state = AppState::new("EUR".to_string());
        *state.file_path.write().await = Some("positions.json.gpg".to_string());
        let err = state.save().await.unwrap_err();
        assert!(err.to_string().contains("encrypted"));
        assert!(!state.can_save().await);
    }

    #[tokio::test]
    async fn test_save_writes_atomically_with_one_backup() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("positions.json");
        std::fs::write(&path, b"old").unwrap();

        let state = AppState::new("EUR".to_string());
        *state.file_path.write().await = Some(path.to_str().unwrap().to_string());
        *state.positions_json.write().await = "new".to_string();
        state.save().await.unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(
            std::fs::read(tmp.path().join("positions.json.bak")).unwrap(),
            b"old"
        );
    }

    #[tokio::test]
    async fn test_save_assigns_stable_position_ids() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("positions.json");
        let legacy = r#"[{"Name": "Cash", "AssetClass": "Cash", "Amount": 100}]"#;
        std::fs::write(&path, legacy).unwrap();

        let state = AppState::new("EUR".to_string());
        *state.file_path.write().await = Some(path.to_str().unwrap().to_string());
        *state.positions_json.write().await = legacy.to_string();
        state.save().await.unwrap();

        let read_ids = || {
            let saved: Vec<PortfolioPosition> =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            saved
                .iter()
                .map(|p| p.id().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        let first = read_ids();
        assert!(first[0].starts_with("pos_"));

        state.save().await.unwrap();
        assert_eq!(read_ids(), first, "ids must not change on later writes");
    }

    #[tokio::test]
    async fn test_add_update_delete_position_roundtrip() {
        let state = AppState::new("EUR".to_string());
        let dto = state
            .add_position(CreatePositionRequest {
                name: Some("Cash".to_string()),
                ticker: None,
                asset_class: "Cash".to_string(),
                amount: 1000.0,
                purchases: vec![],
            })
            .await
            .unwrap();
        assert_eq!(dto.id, 0);
        assert_eq!(dto.amount, 1000.0);

        let dto = state
            .update_position(
                0,
                UpdatePositionRequest {
                    amount: Some(2000.0),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(dto.amount, 2000.0);

        state.delete_position(0).await.unwrap();
        assert!(state.delete_position(0).await.is_err());
    }

    #[tokio::test]
    async fn test_performance_cash_is_neither_gain_nor_loss() {
        use crate::position::PriceStatus;

        let mut stock =
            PortfolioPosition::for_test(Some("FAKE"), "Stocks", 10.0, 80.0, PriceStatus::Live);
        stock.purchases.push(Purchase {
            date: None,
            quantity: 10.0,
            price: Some(100.0),
            fees: None,
        });
        // Balance would count as profit under total-value-minus-invested; the
        // purchase would count as a loss if cash cost basis were included.
        let mut cash = PortfolioPosition::for_test(None, "Cash", 5_000.0, 0.0, PriceStatus::Live);
        cash.purchases.push(Purchase {
            date: None,
            quantity: 1.0,
            price: Some(8_000.0),
            fees: None,
        });

        let mut portfolio = Portfolio::new();
        portfolio.add_position(stock);
        portfolio.add_position(cash);

        let state = AppState::new("EUR".to_string());
        *state.portfolio_cache.write().await = Some(CachedPortfolio {
            portfolio,
            fetched_at: Instant::now(),
        });

        let perf = state.get_performance().await;
        assert_eq!(perf.total_value, 5_800.0);
        assert_eq!(perf.cash_value, 5_000.0);
        assert_eq!(perf.securities_value, 800.0);
        assert_eq!(perf.invested, 1_000.0);
        assert_eq!(perf.unrealized_pnl, -200.0);
        assert!((perf.pnl_percent - (-20.0)).abs() < 1e-9);
    }
}
