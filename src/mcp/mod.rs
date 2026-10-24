//! Local stdio MCP (Model Context Protocol) server: `portfolio_rs mcp`.
//!
//! Read-only Phase 1 tools (`get_positions`, `get_allocation`,
//! `check_policy`, `get_transactions`, `list_portfolios`, `get_context`) and
//! resources over [rmcp](https://docs.rs/rmcp). stdout carries only the
//! JSON-RPC stream; diagnostics go to stderr. Quotes follow
//! `PORTFOLIO_RS_QUOTES` (`--offline` forces offline unless a fixture is set).

pub mod analytics;
pub mod dto;
mod server;
pub mod stdio;

pub use server::PortfolioServer;

use crate::services::quote_cache::{cache_path_for, CachedProvider, DEFAULT_MAX_AGE_DAYS};
use crate::services::quotes::AnyProvider;
use crate::state::AppState;
use eyre::{bail, Result, WrapErr};
use rmcp::ServiceExt;
use std::path::{Path, PathBuf};

/// How the served portfolio is selected.
#[derive(Debug, Clone, Default)]
pub struct McpOptions {
    /// Id clients use in `portfolioId` and resource URIs. Default `main`.
    pub portfolio_id: Option<String>,
    /// Positions file (plaintext or `.gpg`).
    pub file: Option<String>,
    /// Workspace directory (positions.json + portfolio/policy.toml).
    pub workspace: Option<String>,
    /// Policy file for file mode.
    pub policy: Option<String>,
    /// Force offline quotes (a configured fixture still applies).
    pub offline: bool,
    /// Base currency.
    pub currency: String,
    /// Explicit quote provider (tests, embedders). Overrides `offline` and
    /// `PORTFOLIO_RS_QUOTES` when set.
    pub quotes: Option<AnyProvider>,
    /// Last-known-price cache next to the data file.
    pub cache: CacheMode,
    /// Max age of cached prices in days (default 7).
    pub cache_max_age_days: Option<i64>,
    /// Prices older than this many days are stale (default 3).
    pub stale_after_days: Option<i64>,
    /// Drop possible_breach findings when the excluded positions can be at
    /// most this percent of the total (default 0.5).
    pub possible_breach_min_share: Option<f64>,
}

/// Whether the server uses the last-known-price cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CacheMode {
    /// On for Yahoo and offline quotes, off for fixtures and `.gpg` files.
    #[default]
    Auto,
    /// Never read or write the cache (`--no-cache`).
    Off,
    /// Also for fixtures (tests). Never for `.gpg` files.
    On,
}

/// A loaded portfolio and everything the tools need about it.
pub struct ServedPortfolio {
    pub id: String,
    pub label: String,
    pub mode: &'static str,
    pub path: String,
    pub workspace_dir: Option<PathBuf>,
    pub state: AppState,
    /// Why the policy could not be loaded, if it could not.
    pub policy_error: Option<String>,
    /// Whether quotes may come from the network (false with --offline or a fixture).
    pub network: bool,
    /// yahoo | offline | fixture
    pub quote_source: &'static str,
    /// The last-known-price cache file, when enabled.
    pub cache_path: Option<PathBuf>,
    pub policy_options: analytics::PolicyOptions,
}

impl ServedPortfolio {
    pub fn encrypted(&self) -> bool {
        self.path.ends_with(".gpg")
    }

    /// Workspace `INVESTMENT_POLICY.md`, if present.
    pub fn policy_md_path(&self) -> Option<PathBuf> {
        self.workspace_dir
            .as_ref()
            .map(|d| d.join("INVESTMENT_POLICY.md"))
            .filter(|p| p.is_file())
    }
}

fn quote_provider(offline: bool) -> AnyProvider {
    let configured = AnyProvider::configured().clone();
    match configured {
        AnyProvider::Fixture(_) => configured,
        _ if offline => AnyProvider::Offline,
        other => other,
    }
}

/// Load the portfolio selected by `opts`. Never writes the app config.
pub async fn load(opts: &McpOptions) -> Result<ServedPortfolio> {
    let id = opts
        .portfolio_id
        .clone()
        .unwrap_or_else(|| "main".to_string());
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("invalid portfolio id {id:?} (use letters, digits, - and _)");
    }
    let provider = opts
        .quotes
        .clone()
        .unwrap_or_else(|| quote_provider(opts.offline));
    let network = matches!(provider, AnyProvider::Yahoo);
    let quote_source = provider.source_name();
    let data_file = match (&opts.workspace, &opts.file) {
        (Some(dir), _) => Some(Path::new(dir).join("positions.json")),
        (None, Some(file)) => Some(PathBuf::from(file)),
        _ => None,
    };
    let use_cache = match opts.cache {
        CacheMode::Off => false,
        CacheMode::On => true,
        CacheMode::Auto => !matches!(provider, AnyProvider::Fixture(_)),
    };
    // Never a plaintext list of tickers next to an encrypted portfolio.
    let cache_path = data_file
        .as_deref()
        .filter(|f| use_cache && !f.to_string_lossy().ends_with(".gpg"))
        .and_then(cache_path_for);
    let provider = match &cache_path {
        Some(path) => {
            let write = !matches!(provider, AnyProvider::Offline);
            AnyProvider::Cached(std::sync::Arc::new(CachedProvider::new(
                provider,
                Some(path.clone()),
                opts.cache_max_age_days.unwrap_or(DEFAULT_MAX_AGE_DAYS),
                write,
            )))
        }
        None => provider,
    };
    let state = AppState::with_quotes(opts.currency.clone(), provider).with_stale_after(
        opts.stale_after_days
            .unwrap_or(crate::position::DEFAULT_STALE_AFTER_DAYS),
    );
    let mut policy_error = None;
    let (mode, path, workspace_dir) = if let Some(dir) = &opts.workspace {
        let positions = Path::new(dir).join("positions.json");
        if !positions.is_file() {
            bail!(
                "workspace does not contain positions.json: {}",
                positions.display()
            );
        }
        let positions = positions.to_string_lossy().into_owned();
        state.load_file(&positions).await?;
        *state.workspace_dir.write().await = Some(dir.clone());
        if Path::new(dir).join("portfolio/policy.toml").is_file() {
            if let Err(e) = state.load_policy(None).await {
                policy_error = Some(format!("{e:#}"));
            }
        }
        ("workspace", positions, Some(PathBuf::from(dir)))
    } else if let Some(file) = &opts.file {
        state
            .load_file(file)
            .await
            .wrap_err("cannot load the portfolio for the MCP server")?;
        if let Some(policy) = &opts.policy {
            if let Err(e) = state.load_policy(Some(policy.clone())).await {
                policy_error = Some(format!("{e:#}"));
            }
        }
        ("simple", file.clone(), None)
    } else {
        bail!("no portfolio selected: pass FILE or --workspace DIR (or configure one)");
    };
    if let Some(err) = &policy_error {
        eprintln!("Warning: policy not loaded: {err}");
    }
    let label = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| id.clone());
    let label = match &workspace_dir {
        Some(d) => d
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(label),
        None => label,
    };
    Ok(ServedPortfolio {
        id,
        label,
        mode,
        path,
        workspace_dir,
        state,
        policy_error,
        network,
        quote_source,
        cache_path,
        policy_options: analytics::PolicyOptions {
            possible_breach_min_share: opts
                .possible_breach_min_share
                .unwrap_or(analytics::DEFAULT_POSSIBLE_BREACH_MIN_SHARE),
        },
    })
}

/// Serve `opts` over stdin/stdout until the client disconnects.
pub async fn serve_stdio(opts: McpOptions) -> Result<()> {
    let served = load(&opts).await?;
    eprintln!(
        "portfolio_rs MCP server: serving portfolio {:?} ({}) on stdio",
        served.id, served.mode
    );
    let (rx, tx, writer) = stdio::filtered(tokio::io::stdin(), tokio::io::stdout());
    let service = PortfolioServer::new(served)
        .serve((rx, tx))
        .await
        .wrap_err("MCP initialization failed")?;
    let quit = service.waiting().await;
    // Let queued responses reach stdout before the process exits.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), writer).await;
    quit.wrap_err("MCP server stopped with an error")?;
    Ok(())
}
