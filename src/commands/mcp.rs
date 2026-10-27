//! `portfolio_rs mcp`: thin CLI wrapper around [`crate::mcp`].

use std::path::{Path, PathBuf};

use clap::ArgMatches;
use eyre::Result;

use crate::config::{AppConfig, AppMode};
use crate::mcp::snapshots::Upsert;
use crate::mcp::{record_snapshot, serve_stdio, CacheMode, McpOptions};

/// An argument that `snapshot` does not define, unlike `mcp`.
fn optional<T: Clone + Send + Sync + 'static>(matches: &ArgMatches, name: &str) -> Option<T> {
    matches.try_get_one::<T>(name).ok().flatten().cloned()
}

/// Options shared by `mcp` and `snapshot`.
fn options(matches: &ArgMatches, cfg: &AppConfig) -> McpOptions {
    let get = |name: &str| matches.get_one::<String>(name).cloned();
    // Explicit FILE / --workspace win; otherwise the configured portfolio.
    let (file, workspace) = match (get("FILE"), get("workspace")) {
        (None, None) => match (&cfg.last_mode, &cfg.workspace_dir) {
            (AppMode::Workspace, Some(dir)) if Path::new(dir).join("positions.json").is_file() => {
                (None, Some(dir.clone()))
            }
            _ => (cfg.effective_portfolio_file(), None),
        },
        explicit => explicit,
    };
    McpOptions {
        portfolio_id: get("portfolio"),
        file,
        workspace,
        policy: optional(matches, "policy"),
        offline: matches.get_flag("offline"),
        currency: cfg.currency.clone(),
        quotes: None,
        cache: if matches.get_flag("no-cache") {
            CacheMode::Off
        } else {
            CacheMode::Auto
        },
        cache_max_age_days: optional(matches, "cache-max-age"),
        stale_after_days: optional(matches, "stale-after"),
        possible_breach_min_share: optional(matches, "possible-breach-min-share"),
        gain_factors: matches
            .try_get_many::<(String, Option<f64>)>("gain-factor")
            .ok()
            .flatten()
            .map(|v| v.cloned().collect())
            .unwrap_or_default(),
        data_dir: get("data-dir").map(PathBuf::from),
    }
}

pub async fn handle_mcp(matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    serve_stdio(options(matches, cfg)).await
}

/// `portfolio_rs snapshot`: value the portfolio and upsert today's line.
pub async fn handle_snapshot(matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let (path, outcome, complete) = match record_snapshot(&options(matches, cfg)).await {
        Ok(recorded) => recorded,
        Err(e) => match e.downcast_ref::<crate::mcp::PositionsParseError>() {
            // Same exit code as `validate`; nothing was written.
            Some(parse) => {
                eprintln!("{parse}");
                std::process::exit(2);
            }
            None => return Err(e),
        },
    };
    match outcome {
        Upsert::Written if complete => eprintln!("Snapshot written to {}", path.display()),
        Upsert::Written => eprintln!(
            "Snapshot written to {} (incomplete: some prices or FX rates are missing)",
            path.display()
        ),
        Upsert::KeptComplete => eprintln!(
            "Warning: today's snapshot would be incomplete; kept the complete one already in {}",
            path.display()
        ),
    }
    Ok(())
}
