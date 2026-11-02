//! [`AppState`] loaders that persist the choice to the user's real config file.
//!
//! Excluded from the coverage gate (`*_io.rs`): these write the config file in
//! the user's config directory ([`AppConfig::save`]), which tests must never
//! touch. Each one only passes that writer to a `*_with` loader in `state.rs`,
//! which is tested with a writer into a temp dir.

use eyre::Result;

use super::AppState;
use crate::AppConfig;

impl AppState {
    /// Load a workspace directory, reading `positions.json` and `portfolio/policy.toml`.
    pub async fn load_workspace(&self, dir: &str, cfg: &mut AppConfig) -> Result<()> {
        self.load_workspace_with(dir, cfg, AppConfig::save).await
    }

    /// Remember a standalone positions file and clear any workspace.
    pub async fn load_simple_file(&self, path: &str, cfg: &mut AppConfig) -> Result<()> {
        self.load_simple_file_with(path, cfg, AppConfig::save).await
    }
}
