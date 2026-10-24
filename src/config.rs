//! Persistent application configuration.
//!
//! `AppConfig` is the single source of truth for user-level settings used by
//! the CLI, TUI, and HTTP API. It is stored by `confy` under the
//! application's config directory and is versioned so future migrations are
//! straightforward.

use eyre::{eyre, Result, WrapErr};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use crate::theme::ThemeMode;

const APP_NAME: &str = "portfolio";
const CONFIG_NAME: &str = "config";
const CURRENT_CONFIG_VERSION: u32 = 1;

/// Placeholder default from the legacy config. We treat it as unset.
const LEGACY_PLACEHOLDER_FILE: &str = "/home/Joe/portfolio.json";

/// Write `cfg` to `path` with owner-only permissions on unix.
fn store_private(path: &Path, cfg: &AppConfig) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        confy::store_path_perms(path, cfg, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    confy::store_path(path, cfg)?;
    Ok(())
}

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

/// Configured base currency, read without side effects.
///
/// Unlike [`AppConfig::load`], this never creates, migrates, or writes the
/// config file. Returns `"EUR"` when no config file exists or it cannot be read.
pub fn configured_base_currency() -> String {
    const DEFAULT: &str = "EUR";
    let Ok(path) = confy::get_configuration_file_path(APP_NAME, CONFIG_NAME) else {
        return DEFAULT.to_string();
    };
    if !path.is_file() {
        return DEFAULT.to_string();
    }
    match confy::load_path::<AppConfig>(&path) {
        Ok(cfg) if !cfg.currency.trim().is_empty() => cfg.currency,
        _ => DEFAULT.to_string(),
    }
}

impl AppConfig {
    /// Load the config from disk, migrating from older formats if needed.
    ///
    /// A missing file is created with defaults. A file that exists but cannot
    /// be parsed (as the current or the legacy format) is an error and is left
    /// untouched, so settings such as `llm_api_key` are never wiped.
    pub fn load() -> Result<Self> {
        let path = confy::get_configuration_file_path(APP_NAME, CONFIG_NAME)
            .wrap_err("failed to get configuration file path")?;
        Self::load_from(&path)
    }

    /// [`AppConfig::load`] for an explicit path.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            let cfg = AppConfig::default();
            cfg.save_to(path)?;
            return Ok(cfg);
        }

        let current_err = match confy::load_path::<AppConfig>(path) {
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
                    cfg.save_to(path)?;
                }

                return Ok(cfg);
            }
            Err(err) => err,
        };

        // Try to load a legacy config and migrate it.
        if let Ok(legacy) = confy::load_path::<LegacyConfig>(path) {
            let mut cfg = AppConfig::default();
            cfg.set_currency(legacy.currency);
            if !legacy.portfolio_file.is_empty() && legacy.portfolio_file != LEGACY_PLACEHOLDER_FILE
            {
                cfg.portfolio_file = Some(legacy.portfolio_file);
            }
            cfg.save_to(path)?;
            return Ok(cfg);
        }

        Err(eyre!(
            "configuration file {} could not be parsed ({}). It was left unchanged; \
             fix or move it away and run again",
            path.display(),
            current_err
        ))
    }

    /// Persist the config to disk.
    pub fn save(&self) -> Result<()> {
        let path = confy::get_configuration_file_path(APP_NAME, CONFIG_NAME)
            .wrap_err("failed to get configuration file path")?;
        self.save_to(&path)
    }

    /// [`AppConfig::save`] for an explicit path.
    ///
    /// Writes a temp file in the same directory and renames it over `path`,
    /// so a failed write never truncates the existing config. On unix the
    /// file is always owner-only (0600) because it can hold an API key.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| eyre!("config path has no parent: {}", path.display()))?;
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("failed to create config dir: {}", parent.display()))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| eyre!("config path has no file name: {}", path.display()))?;
        let tmp = parent.join(format!(
            ".{}.tmp-{}",
            file_name.to_string_lossy(),
            std::process::id()
        ));
        // Create the temp file owner-only before any content is written.
        drop(
            crate::services::atomic_write::private_options()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)
                .wrap_err_with(|| format!("failed to create {}", tmp.display()))?,
        );
        let result = store_private(&tmp, self)
            .and_then(|()| std::fs::rename(&tmp, path).wrap_err("failed to replace config file"));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result.wrap_err("failed to save configuration")
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

    /// Set the LLM API key.
    ///
    /// An empty or whitespace-only string is the explicit clear: the stored
    /// key becomes `None`. A missing or null `llmApiKey` on
    /// [`crate::state::AppConfigDto`] is not a clear — see
    /// [`crate::state::AppConfigDto::apply_to`].
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

    #[test]
    fn test_unparsable_config_is_an_error_and_left_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yml");
        let broken = "currency: [EUR\nllm_api_key: fake-key-not-real\n";
        std::fs::write(&path, broken).unwrap();
        let err = AppConfig::load_from(&path).unwrap_err();
        assert!(format!("{err:#}").contains("could not be parsed"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    fn test_save_and_load_roundtrip_is_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("config.yml");
        let cfg = AppConfig::load_from(&path).unwrap();
        assert_eq!(cfg.currency, "EUR");
        let mut cfg = cfg;
        cfg.llm_api_key = Some("fake-key-not-real".to_string());
        cfg.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded.llm_api_key.as_deref(), Some("fake-key-not-real"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "{leftovers:?}");
    }

    #[test]
    fn test_legacy_config_is_migrated_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yml");
        std::fs::write(&path, "portfolio_file: /tmp/fake.json\ncurrency: USD\n").unwrap();
        let cfg = AppConfig::load_from(&path).unwrap();
        assert_eq!(cfg.currency, "USD");
        assert_eq!(cfg.portfolio_file.as_deref(), Some("/tmp/fake.json"));
        let again = AppConfig::load_from(&path).unwrap();
        assert_eq!(again.currency, "USD");
    }
}
