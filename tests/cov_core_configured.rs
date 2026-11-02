//! The process-wide quote provider chosen by `PORTFOLIO_RS_QUOTES`.
//!
//! A separate test binary: the provider is resolved once per process, so
//! every test here first points the variable at the offline quote fixture.

use std::sync::Once;

use chrono::{TimeZone, Utc};
use portfolio_rs::position::handle_position;
use portfolio_rs::{from_string, AppState, Portfolio};

fn use_fixture_quotes() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/quotes_example.json"
        );
        std::env::set_var("PORTFOLIO_RS_QUOTES", format!("fixture:{fixture}"));
    });
}

/// Two SPY (fixture: latest 500 USD, historic close 400) via the configured provider.
async fn spy_portfolio() -> Portfolio {
    let mut positions =
        from_string(r#"[{"Ticker": "SPY", "AssetClass": "Stocks", "Amount": 2}]"#).unwrap();
    let position = handle_position(&mut positions[0]).await.unwrap();
    assert_eq!(position.get_name(), "Fake S&P Fund");
    assert_eq!(position.market_price(), 500.0);
    let mut portfolio = Portfolio::new();
    portfolio.add_position(position);
    portfolio
}

#[tokio::test]
async fn historic_values_use_the_configured_provider() {
    use_fixture_quotes();
    let portfolio = spy_portfolio().await;
    let date = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    assert_eq!(
        portfolio
            .historic_and_current_securities_value(date)
            .await
            .unwrap(),
        (800.0, 1000.0)
    );
    assert_eq!(
        portfolio.get_historic_securities_value(date).await.unwrap(),
        800.0
    );
}

#[tokio::test]
async fn app_state_new_uses_the_configured_provider() {
    use_fixture_quotes();
    let state = AppState::new("USD".to_string());
    state
        .set_positions_json(r#"[{"Ticker": "SPY", "AssetClass": "Stocks", "Amount": 2}]"#.into())
        .await;
    let positions = state.get_positions().await;
    assert_eq!(positions[0].price, 500.0);
    assert_eq!(positions[0].value, 1000.0);
    assert_eq!(positions[0].name.as_deref(), Some("Fake S&P Fund"));
}
