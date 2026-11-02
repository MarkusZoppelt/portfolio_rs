//! `AppConfig::load/save/path` and `configured_base_currency` find the user
//! config dir through the environment. This binary holds a single test, so
//! pointing HOME/XDG_CONFIG_HOME at a tempdir cannot race other tests and
//! the real `~/.config/portfolio` is never touched.

use portfolio_rs::config::configured_base_currency;
use portfolio_rs::{config_path, load_config, AppConfig};

#[test]
fn config_lives_in_the_user_config_dir() {
    let home = tempfile::TempDir::new().unwrap();
    std::env::set_var("HOME", home.path());
    std::env::set_var("XDG_CONFIG_HOME", home.path().join(".config"));
    let expected = home.path().join(".config/portfolio/config.yml");

    assert_eq!(config_path().unwrap(), expected.to_str().unwrap());
    assert_eq!(AppConfig::path().unwrap(), config_path().unwrap());

    // No file yet: the default currency, and reading creates nothing.
    assert_eq!(configured_base_currency(), "EUR");
    assert!(!expected.exists());

    // Loading creates the default file.
    let mut cfg = load_config().unwrap();
    assert!(expected.is_file());
    assert_eq!(cfg.currency, "EUR");

    cfg.set_currency("USD");
    cfg.save().unwrap();
    assert_eq!(configured_base_currency(), "USD");
    assert_eq!(AppConfig::load().unwrap().currency, "USD");
}
