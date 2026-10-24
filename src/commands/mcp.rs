//! `portfolio_rs mcp`: thin CLI wrapper around [`crate::mcp`].

use crate::config::AppConfig;
use crate::mcp::{serve_stdio, McpOptions};
use clap::ArgMatches;
use eyre::Result;

fn options(matches: &ArgMatches, cfg: &AppConfig) -> McpOptions {
    let get = |name: &str| matches.get_one::<String>(name).cloned();
    let workspace = get("workspace");
    // Explicit FILE / --workspace win; otherwise the configured portfolio.
    let (file, workspace) = match (get("FILE"), workspace) {
        (None, None) => match (&cfg.last_mode, &cfg.workspace_dir) {
            (crate::config::AppMode::Workspace, Some(dir))
                if std::path::Path::new(dir).join("positions.json").is_file() =>
            {
                (None, Some(dir.clone()))
            }
            _ => (cfg.effective_portfolio_file(), None),
        },
        (file, ws) => (file, ws),
    };
    McpOptions {
        portfolio_id: get("portfolio"),
        file,
        workspace,
        policy: matches
            .try_get_one::<String>("policy")
            .ok()
            .flatten()
            .cloned(),
        offline: matches.get_flag("offline"),
        currency: cfg.currency.clone(),
        quotes: None,
        cache: if matches.get_flag("no-cache") {
            crate::mcp::CacheMode::Off
        } else {
            crate::mcp::CacheMode::Auto
        },
        cache_max_age_days: matches
            .try_get_one::<i64>("cache-max-age")
            .ok()
            .flatten()
            .copied(),
        stale_after_days: matches
            .try_get_one::<i64>("stale-after")
            .ok()
            .flatten()
            .copied(),
        possible_breach_min_share: matches
            .try_get_one::<f64>("possible-breach-min-share")
            .ok()
            .flatten()
            .copied(),
        data_dir: get("data-dir").map(std::path::PathBuf::from),
    }
}

pub async fn handle_mcp(matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    serve_stdio(options(matches, cfg)).await
}

/// `portfolio_rs snapshot`: value the portfolio and upsert today's line.
pub async fn handle_snapshot(matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
    let opts = options(matches, cfg);
    use crate::mcp::snapshots::Upsert;
    let (path, outcome, complete) = crate::mcp::record_snapshot(&opts).await?;
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
