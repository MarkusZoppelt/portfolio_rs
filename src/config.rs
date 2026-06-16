//! Persistent application configuration.
//!
//! `AppConfig` is the single source of truth for user-level settings used by
//! the CLI, TUI, and HTTP API. It is stored by `confy` under the
//! application's config directory and is versioned so future migrations are
//! straightforward.

use eyre::{eyre, Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use crate::theme::ThemeMode;

const APP_NAME: &str = "portfolio";
const CONFIG_NAME: &str = "config";
const CURRENT_CONFIG_VERSION: u32 = 1;

/// Placeholder default from the legacy config. We treat it as unset.
const LEGACY_PLACEHOLDER_FILE: &str = "/home/Joe/portfolio.json";

/// Legacy configuration shape before versioned `AppConfig`.
#[derive(Debug, Serialize, Deserialize)]
struct LegacyConfig {
    pub portfolio_file: String,
    pub currency: String,
}

impl Default for LegacyConfig {
    fn default() -> Self {
        Self {
            portfolio_file: LEGACY_PLACEHOLDER_FILE.to_string(),
            currency: "EUR".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppMode {
    /// Single positions.json file, no workspace directories.
    #[default]
    Simple,
    /// Full workspace with portfolio/policy.toml, diary, decisions, etc.
    Workspace,
}

impl AppMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AppMode::Simple => "simple",
            AppMode::Workspace => "workspace",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub version: u32,
    pub currency: String,
    pub theme: ThemeMode,
    pub last_mode: AppMode,
    pub portfolio_file: Option<String>,
    pub workspace_dir: Option<String>,
    pub llm_provider_url: Option<String>,
    pub llm_api_key: Option<String>,
    pub llm_model: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_CONFIG_VERSION,
            currency: "EUR".to_string(),
            theme: ThemeMode::Dark,
            last_mode: AppMode::Simple,
            portfolio_file: None,
            workspace_dir: None,
            llm_provider_url: None,
            llm_api_key: None,
            llm_model: None,
        }
    }
}

impl AppConfig {
    /// Load the config from disk, migrating from older formats if needed.
    pub fn load() -> Result<Self> {
        match confy::load::<AppConfig>(APP_NAME, CONFIG_NAME) {
            Ok(mut cfg) => {
                let mut dirty = false;

                // Migrate legacy placeholder file to None.
                if let Some(file) = &cfg.portfolio_file {
                    if file == LEGACY_PLACEHOLDER_FILE || file.trim().is_empty() {
                        cfg.portfolio_file = None;
                        dirty = true;
                    }
                }

                // If a workspace is set, prefer it and drop a stale simple file.
                if cfg.workspace_dir.is_some() && cfg.portfolio_file.is_some() {
                    cfg.portfolio_file = None;
                    dirty = true;
                }

                cfg.version = CURRENT_CONFIG_VERSION;

                if dirty {
                    cfg.save()?;
                }

                Ok(cfg)
            }
            Err(_) => {
                // Try to load a legacy config and migrate it.
                if let Ok(legacy) = confy::load::<LegacyConfig>(APP_NAME, CONFIG_NAME) {
                    let mut cfg = AppConfig::default();
                    cfg.set_currency(legacy.currency);
                    if !legacy.portfolio_file.is_empty()
                        && legacy.portfolio_file != LEGACY_PLACEHOLDER_FILE
                    {
                        cfg.portfolio_file = Some(legacy.portfolio_file);
                    }
                    cfg.save()?;
                    return Ok(cfg);
                }

                // Last resort: start fresh.
                let cfg = AppConfig::default();
                cfg.save()?;
                Ok(cfg)
            }
        }
    }

    /// Persist the config to disk.
    pub fn save(&self) -> Result<()> {
        confy::store(APP_NAME, CONFIG_NAME, self).wrap_err("failed to save configuration")?;
        Ok(())
    }

    /// Return the path to the configuration file.
    pub fn path() -> Result<String> {
        let path = confy::get_configuration_file_path(APP_NAME, CONFIG_NAME)
            .wrap_err("failed to get configuration file path")?;
        path.to_str()
            .map(String::from)
            .ok_or_else(|| eyre!("configuration path contains invalid UTF-8"))
    }

    /// Resolve the effective portfolio file path.
    ///
    /// - In workspace mode, returns `<workspace_dir>/positions.json` if the
    ///   workspace exists.
    /// - In simple mode, returns the remembered portfolio file.
    /// - Falls back to the legacy config value if present.
    pub fn effective_portfolio_file(&self) -> Option<String> {
        if self.last_mode == AppMode::Workspace {
            if let Some(dir) = &self.workspace_dir {
                let path = PathBuf::from(dir).join("positions.json");
                if path.exists() {
                    return path.to_str().map(String::from);
                }
            }
        }
        self.portfolio_file.as_ref().and_then(|f| {
            if f == LEGACY_PLACEHOLDER_FILE || f.trim().is_empty() {
                None
            } else {
                Some(f.clone())
            }
        })
    }

    /// Set the active workspace and clear any standalone file.
    pub fn set_workspace(&mut self, path: impl Into<String>) {
        self.workspace_dir = Some(path.into());
        self.portfolio_file = None;
        self.last_mode = AppMode::Workspace;
    }

    /// Set a standalone portfolio file and clear any workspace.
    pub fn set_portfolio_file(&mut self, path: impl Into<String>) {
        self.portfolio_file = Some(path.into());
        self.workspace_dir = None;
        self.last_mode = AppMode::Simple;
    }

    pub fn set_currency(&mut self, currency: impl Into<String>) {
        self.currency = currency.into();
    }

    pub fn set_theme(&mut self, theme: ThemeMode) {
        self.theme = theme;
    }

    pub fn set_last_mode(&mut self, mode: AppMode) {
        self.last_mode = mode;
    }

    pub fn set_llm_provider_url(&mut self, url: impl Into<String>) {
        let url = url.into();
        self.llm_provider_url = if url.trim().is_empty() {
            None
        } else {
            Some(url)
        };
    }

    pub fn set_llm_api_key(&mut self, key: impl Into<String>) {
        let key = key.into();
        self.llm_api_key = if key.trim().is_empty() {
            None
        } else {
            Some(key)
        };
    }

    pub fn set_llm_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        self.llm_model = if model.trim().is_empty() {
            None
        } else {
            Some(model)
        };
    }

    /// Clear all remembered paths and return to onboarding state.
    pub fn clear_paths(&mut self) {
        self.portfolio_file = None;
        self.workspace_dir = None;
        self.last_mode = AppMode::Simple;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.version, CURRENT_CONFIG_VERSION);
        assert_eq!(cfg.currency, "EUR");
        assert_eq!(cfg.theme, ThemeMode::Dark);
        assert_eq!(cfg.last_mode, AppMode::Simple);
        assert!(cfg.portfolio_file.is_none());
        assert!(cfg.workspace_dir.is_none());
        assert!(cfg.llm_provider_url.is_none());
        assert!(cfg.llm_api_key.is_none());
        assert!(cfg.llm_model.is_none());
    }

    #[test]
    fn test_migrate_legacy_placeholder() {
        let cfg = AppConfig {
            portfolio_file: Some(LEGACY_PLACEHOLDER_FILE.to_string()),
            ..Default::default()
        };
        // effective_portfolio_file should treat it as unset.
        assert!(cfg.effective_portfolio_file().is_none());
    }

    #[test]
    fn test_workspace_takes_precedence() {
        let mut cfg = AppConfig::default();
        cfg.set_workspace("/tmp/ws");
        // Without an actual positions.json we fall back to the file only if set.
        assert!(cfg.portfolio_file.is_none());
        assert_eq!(cfg.last_mode, AppMode::Workspace);
    }
}
