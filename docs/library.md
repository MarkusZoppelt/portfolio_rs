# Library usage

The crate is also a library: `state::AppState` is an embedding-friendly
facade used by the HTTP API and external GUIs (e.g. a Tauri desktop app).
It loads portfolios/workspaces/policies, does position CRUD, and runs every
analysis (context, review, simulate, doctor, validate, reports) with
camelCase-serializable DTOs.

```rust
use portfolio_rs::AppState;

async fn total_value() -> eyre::Result<f64> {
    let state = AppState::new("EUR".to_string());
    state.load_file("positions.json").await?;
    let summary = state.get_portfolio_summary().await;
    Ok(summary.total_value)
}
```

