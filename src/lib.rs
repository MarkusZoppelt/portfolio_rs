//! portfolio_rs — local-first portfolio management for humans and AI agents.
//!
//! This crate is both a binary (CLI + TUI) and a library. The library layer
//! powers the CLI, the HTTP API server, and external GUIs (e.g. a Tauri app).
//!
//! # Entry points
//!
//! - [`state::AppState`]: embedding-friendly facade used by GUIs and the
//!   HTTP API — load portfolios/workspaces/policies, CRUD positions, and run
//!   analyses (context, review, simulate, doctor, validate, reports).
//! - [`api`]: the local Axum HTTP server (`portfolio_rs api`).
//! - [`cli`]: clap command definitions and dispatch for the binary.
//! - Domain modules: [`portfolio`], [`position`], [`policy`], [`review`],
//!   [`simulate`], [`context`], [`doctor`], [`validate`], [`workspace`].
//!
//! All DTOs in [`state`] serialize as camelCase JSON for direct consumption
//! by web frontends.

use eyre::{Result, WrapErr};

pub mod agent_skill;
pub mod api;
pub mod cli;
pub mod commands;
pub mod config;
pub mod context;
pub mod decision;
pub mod doctor;
pub mod document;
pub mod error;
pub mod policy;
pub mod portfolio;
pub mod position;
pub mod report;
pub mod review;
pub mod services;
pub mod simulate;
pub mod state;
pub mod theme;
pub mod tui;
pub mod validate;
pub mod workspace;

pub use config::{AppConfig, AppMode, ThemeMode};
pub use context::PortfolioContext;
pub use doctor::WorkspaceHealth;
pub use policy::Policy;
pub use portfolio::Portfolio;
pub use position::{from_string, PortfolioPosition, Purchase};
pub use review::Review;
pub use services::portfolio_loader::{create_live_portfolio, load_portfolio_file};
pub use simulate::{simulate_rebalance, RebalanceSimulation};
pub use state::AppState;
pub use tui::NetworkStatus;
pub use validate::ValidationReport;

/// Load the application configuration.
pub fn load_config() -> Result<AppConfig> {
    AppConfig::load()
}

/// Returns the path to the configuration file.
pub fn config_path() -> Result<String> {
    AppConfig::path()
}

/// Serialize a list of positions to pretty-printed JSON.
pub fn positions_to_string(positions: &[PortfolioPosition]) -> Result<String> {
    serde_json::to_string_pretty(positions).wrap_err("failed to serialize positions")
}
