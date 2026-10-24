//! `portfolio_rs mcp`: thin CLI wrapper around [`crate::mcp`].

use crate::config::AppConfig;
use crate::mcp::{serve_stdio, McpOptions};
use clap::ArgMatches;
use eyre::Result;

pub async fn handle_mcp(matches: &ArgMatches, cfg: &AppConfig) -> Result<()> {
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
    serve_stdio(McpOptions {
        portfolio_id: get("portfolio"),
        file,
        workspace,
        policy: get("policy"),
        offline: matches.get_flag("offline"),
        currency: cfg.currency.clone(),
        quotes: None,
    })
    .await
}
