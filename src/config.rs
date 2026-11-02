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
    config_file_path()
        .ok()
        .and_then(|path| base_currency_at(&path))
        .unwrap_or_else(|| "EUR".to_string())
}

/// The non-empty currency of the config file at `path`, if it exists and
/// parses. Never creates the file.
fn base_currency_at(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let cfg = confy::load_path::<AppConfig>(path).ok()?;
    Some(cfg.currency).filter(|c| !c.trim().is_empty())
}

/// Where confy keeps the config file (under the user config dir).
fn config_file_path() -> Result<PathBuf> {
    confy::get_configuration_file_path(APP_NAME, CONFIG_NAME)
        .wrap_err("failed to get configuration file path")
}

/// `path` as a string; errors on invalid UTF-8.
fn path_to_utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(String::from)
        .ok_or_else(|| eyre!("configuration path contains invalid UTF-8"))
}

impl AppConfig {
    /// Load the config from disk, migrating from older formats if needed.
    ///
    /// A missing file is created with defaults. A file that exists but cannot
    /// be parsed (as the current or the legacy format) is an error and is left
    /// untouched, so settings such as `llm_api_key` are never wiped.
    pub fn load() -> Result<Self> {
        Self::load_from(&config_file_path()?)
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
        self.save_to(&config_file_path()?)
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
        path_to_utf8(&config_file_path()?)
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
    fn test_app_mode_as_str() {
        assert_eq!(AppMode::Simple.as_str(), "simple");
        assert_eq!(AppMode::Workspace.as_str(), "workspace");
    }

    /// Write `cfg` to `path` without the migrations `load_from` applies.
    fn write_raw(path: &Path, cfg: &AppConfig) {
        store_private(path, cfg).unwrap();
    }

    #[test]
    fn test_blank_or_placeholder_file_is_migrated_to_unset_and_saved() {
        for file in ["", "   ", LEGACY_PLACEHOLDER_FILE] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("config.yml");
            let stale = AppConfig {
                version: 0,
                portfolio_file: Some(file.to_string()),
                ..Default::default()
            };
            write_raw(&path, &stale);
            let cfg = AppConfig::load_from(&path).unwrap();
            assert_eq!(cfg.portfolio_file, None, "{file:?}");
            assert_eq!(cfg.version, CURRENT_CONFIG_VERSION);
            // The migration was persisted.
            let raw: AppConfig = confy::load_path(&path).unwrap();
            assert_eq!(raw.portfolio_file, None, "{file:?}");
        }
    }

    #[test]
    fn test_workspace_drops_a_stale_portfolio_file_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yml");
        let stale = AppConfig {
            workspace_dir: Some("/tmp/fake-ws".to_string()),
            portfolio_file: Some("/tmp/fake.json".to_string()),
            ..Default::default()
        };
        write_raw(&path, &stale);
        let cfg = AppConfig::load_from(&path).unwrap();
        assert_eq!(cfg.workspace_dir.as_deref(), Some("/tmp/fake-ws"));
        assert_eq!(cfg.portfolio_file, None);
        let raw: AppConfig = confy::load_path(&path).unwrap();
        assert_eq!(raw.portfolio_file, None);
    }

    #[test]
    fn test_clean_config_is_loaded_without_rewriting() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yml");
        let mut cfg = AppConfig::default();
        cfg.set_portfolio_file("/tmp/fake.json");
        cfg.save_to(&path).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded.portfolio_file.as_deref(), Some("/tmp/fake.json"));
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn test_legacy_default_placeholder_migrates_to_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yml");
        // What confy writes for a fresh legacy config.
        confy::store_path(&path, LegacyConfig::default()).unwrap();
        let cfg = AppConfig::load_from(&path).unwrap();
        assert_eq!(cfg.currency, "EUR");
        assert_eq!(cfg.portfolio_file, None);
        assert_eq!(cfg.version, CURRENT_CONFIG_VERSION);
    }

    #[test]
    fn test_effective_portfolio_file() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_str().unwrap().to_string();

        // Workspace mode without a workspace dir uses the file.
        let cfg = AppConfig {
            last_mode: AppMode::Workspace,
            portfolio_file: Some("/tmp/fake.json".to_string()),
            ..Default::default()
        };
        assert_eq!(
            cfg.effective_portfolio_file().as_deref(),
            Some("/tmp/fake.json")
        );

        let mut cfg = AppConfig::default();
        cfg.set_workspace(ws.clone());
        // Workspace without positions.json and no file: nothing.
        assert_eq!(cfg.effective_portfolio_file(), None);
        // A file kept next to the workspace is the fallback.
        cfg.portfolio_file = Some("/tmp/fake.json".to_string());
        assert_eq!(
            cfg.effective_portfolio_file().as_deref(),
            Some("/tmp/fake.json")
        );
        // Workspace positions.json wins once it exists.
        std::fs::write(tmp.path().join("positions.json"), "[]").unwrap();
        assert_eq!(
            cfg.effective_portfolio_file(),
            Some(format!("{ws}/positions.json"))
        );
        // Simple mode ignores the workspace.
        cfg.set_last_mode(AppMode::Simple);
        assert_eq!(
            cfg.effective_portfolio_file().as_deref(),
            Some("/tmp/fake.json")
        );
        cfg.portfolio_file = Some("  ".to_string());
        assert_eq!(cfg.effective_portfolio_file(), None);
    }

    #[test]
    fn test_setters() {
        let mut cfg = AppConfig::default();
        cfg.set_workspace("/tmp/ws");
        cfg.set_portfolio_file("/tmp/fake.json");
        assert_eq!(cfg.portfolio_file.as_deref(), Some("/tmp/fake.json"));
        assert_eq!(cfg.workspace_dir, None);
        assert_eq!(cfg.last_mode, AppMode::Simple);

        cfg.set_theme(ThemeMode::Light);
        assert_eq!(cfg.theme, ThemeMode::Light);
        cfg.set_last_mode(AppMode::Workspace);
        assert_eq!(cfg.last_mode, AppMode::Workspace);

        cfg.set_llm_provider_url("http://localhost:1234");
        cfg.set_llm_api_key("fake-key-not-real");
        cfg.set_llm_model("fake-model");
        assert_eq!(
            cfg.llm_provider_url.as_deref(),
            Some("http://localhost:1234")
        );
        assert_eq!(cfg.llm_api_key.as_deref(), Some("fake-key-not-real"));
        assert_eq!(cfg.llm_model.as_deref(), Some("fake-model"));
        // Blank values clear.
        cfg.set_llm_provider_url(" ");
        cfg.set_llm_api_key("");
        cfg.set_llm_model("\t");
        assert_eq!(cfg.llm_provider_url, None);
        assert_eq!(cfg.llm_api_key, None);
        assert_eq!(cfg.llm_model, None);

        cfg.set_workspace("/tmp/ws");
        cfg.clear_paths();
        assert_eq!(cfg.workspace_dir, None);
        assert_eq!(cfg.portfolio_file, None);
        assert_eq!(cfg.last_mode, AppMode::Simple);
    }

    #[test]
    fn test_save_to_rejects_paths_without_parent_or_file_name() {
        let cfg = AppConfig::default();
        let err = cfg.save_to(Path::new("/")).unwrap_err();
        assert_eq!(err.to_string(), "config path has no parent: /");

        let tmp = tempfile::tempdir().unwrap();
        let err = cfg.save_to(&tmp.path().join("..")).unwrap_err();
        assert!(
            err.to_string().starts_with("config path has no file name"),
            "{err}"
        );
    }

    #[test]
    fn test_save_to_reports_unwritable_locations() {
        let cfg = AppConfig::default();
        let tmp = tempfile::tempdir().unwrap();

        // The config dir cannot be created below a file.
        let file = tmp.path().join("file");
        std::fs::write(&file, "").unwrap();
        let err = cfg.save_to(&file.join("config.yml")).unwrap_err();
        assert!(
            err.to_string().starts_with("failed to create config dir"),
            "{err}"
        );

        // The temp file cannot be created over a directory.
        let dir = tmp.path().join("blocked");
        std::fs::create_dir_all(dir.join(format!(".config.yml.tmp-{}", std::process::id())))
            .unwrap();
        let err = cfg.save_to(&dir.join("config.yml")).unwrap_err();
        assert!(err.to_string().starts_with("failed to create"), "{err}");
    }

    #[test]
    fn test_failed_replace_keeps_the_target_and_removes_the_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        // A non-empty directory where the config file should go.
        let path = tmp.path().join("config.yml");
        std::fs::create_dir_all(path.join("keep")).unwrap();
        let err = AppConfig::default().save_to(&path).unwrap_err();
        assert_eq!(err.to_string(), "failed to save configuration");
        assert!(path.join("keep").is_dir());
        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("config.yml")]);
    }

    #[test]
    fn test_base_currency_at() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yml");
        assert_eq!(base_currency_at(&path), None);
        assert!(!path.exists(), "reading must never create the file");

        let mut cfg = AppConfig::default();
        cfg.set_currency("USD");
        cfg.save_to(&path).unwrap();
        assert_eq!(base_currency_at(&path).as_deref(), Some("USD"));

        cfg.set_currency(" ");
        cfg.save_to(&path).unwrap();
        assert_eq!(base_currency_at(&path), None);

        std::fs::write(&path, "currency: [EUR\n").unwrap();
        assert_eq!(base_currency_at(&path), None);
    }

    #[cfg(unix)]
    #[test]
    fn test_path_to_utf8() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(
            path_to_utf8(Path::new("/tmp/config.yml")).unwrap(),
            "/tmp/config.yml"
        );
        let bad = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff.yml"));
        assert_eq!(
            path_to_utf8(bad).unwrap_err().to_string(),
            "configuration path contains invalid UTF-8"
        );
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
