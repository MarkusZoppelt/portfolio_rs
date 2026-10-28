//! Offline tests for the HTTP API router.
//!
//! Uses cash-only positions (no tickers) so no live quotes are fetched and
//! the tests are fully deterministic without network access.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use portfolio_rs::api::{
    create_guarded_router, create_router, write_token_file, ApiGuard, ApiState,
};

const CASH_ONLY: &str = r#"[
  {
    "Name": "Cash",
    "AssetClass": "Cash",
    "Amount": 1000.0
  }
]"#;

async fn router_with(
    positions_json: &str,
    file_path: Option<String>,
    policy_file: Option<String>,
) -> axum::Router {
    let state = ApiState::new(
        positions_json.to_string(),
        file_path,
        policy_file,
        "EUR".to_string(),
    )
    .await
    .unwrap();
    create_router(state)
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn json_request(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn test_health() {
    let app = router_with(CASH_ONLY, None, None).await;
    let response = app.oneshot(get("/health")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_portfolio_summary_is_loaded_before_serving() {
    let app = router_with(CASH_ONLY, None, None).await;
    let response = app.oneshot(get("/api/portfolio")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    // No startup race: the initial load happens before the router serves.
    assert_eq!(json["totalValue"], 1000.0);
    assert_eq!(json["cashValue"], 1000.0);
    assert_eq!(json["positions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_get_positions_camel_case() {
    let app = router_with(CASH_ONLY, None, None).await;
    let response = app.oneshot(get("/api/positions")).await.unwrap();
    let json = body_json(response).await;
    assert_eq!(json[0]["assetClass"], "Cash");
    assert!(json[0].get("dayChangePercent").is_some());
}

#[tokio::test]
async fn test_get_missing_position_returns_404_json() {
    let app = router_with(CASH_ONLY, None, None).await;
    let response = app.oneshot(get("/api/positions/42")).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert!(json["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn test_crud_position_persists_to_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let file = tmp.path().join("positions.json");
    std::fs::write(&file, CASH_ONLY).unwrap();
    let file_path = file.to_str().unwrap().to_string();

    let app = router_with(CASH_ONLY, Some(file_path.clone()), None).await;

    // Create
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/positions",
            serde_json::json!({
                "name": "Savings",
                "assetClass": "Cash",
                "amount": 500.0,
                "purchases": []
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = body_json(response).await;
    assert_eq!(created["id"], 1);

    // Update
    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            "/api/positions/1",
            serde_json::json!({ "amount": 750.0 }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["amount"], 750.0);

    // Mutations are persisted to the backing file.
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert!(on_disk.contains("Savings"));
    assert!(on_disk.contains("750"));

    // Delete
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/positions/1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert!(!on_disk.contains("Savings"));

    // Deleting again is a 404.
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/positions/1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_review_without_policy_is_400() {
    let app = router_with(CASH_ONLY, None, None).await;
    let response = app.oneshot(get("/api/review")).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert!(json["error"].as_str().unwrap().contains("policy"));
}

#[tokio::test]
async fn test_review_and_simulate_with_policy() {
    let tmp = tempfile::TempDir::new().unwrap();
    let policy_path = tmp.path().join("policy.toml");
    let policy = portfolio_rs::policy::default_balanced_growth_policy();
    std::fs::write(&policy_path, policy.to_toml().unwrap()).unwrap();
    let policy_file = policy_path.to_str().unwrap().to_string();

    let app = router_with(CASH_ONLY, None, Some(policy_file)).await;

    let response = app.clone().oneshot(get("/api/review")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["policyName"], "Balanced Growth");

    let response = app.oneshot(get("/api/simulate")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["scenarios"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_context_endpoint() {
    let app = router_with(CASH_ONLY, None, None).await;
    let response = app.oneshot(get("/api/context")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(json.get("generatedAt").is_some());
    assert_eq!(json["summary"]["totalValue"], 1000.0);
}

#[tokio::test]
async fn test_validate_endpoint_requires_loaded_file() {
    // Without a backing file there is nothing to validate.
    let app = router_with(CASH_ONLY, None, None).await;
    let response = app.oneshot(get("/api/validate")).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_validate_endpoint_with_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let file = tmp.path().join("positions.json");
    std::fs::write(&file, CASH_ONLY).unwrap();

    let app = router_with(CASH_ONLY, Some(file.to_str().unwrap().to_string()), None).await;
    let response = app.oneshot(get("/api/validate")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["valid"], true);
    assert_eq!(json["positionCount"], 1);
}

#[tokio::test]
async fn test_gpg_backed_portfolio_is_never_rewritten() {
    let tmp = tempfile::TempDir::new().unwrap();
    let file = tmp.path().join("positions.json.gpg");
    std::fs::write(&file, "encrypted-bytes").unwrap();

    let app = router_with(CASH_ONLY, Some(file.to_str().unwrap().to_string()), None).await;
    let response = app
        .oneshot(json_request(
            "POST",
            "/api/positions",
            serde_json::json!({
                "name": "Savings",
                "assetClass": "Cash",
                "amount": 500.0,
                "purchases": []
            }),
        ))
        .await
        .unwrap();
    // Mutation succeeds in memory...
    assert_eq!(response.status(), StatusCode::CREATED);
    // ...but the encrypted file is untouched.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "encrypted-bytes");
}

#[tokio::test]
async fn test_draft_decision_refuses_overwrite() {
    // Two calls with the same title on the same day: second must fail,
    // no file must be silently overwritten. In-memory only (no backing
    // file path) — this is the GUI's exact code path via AppState.
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(workspace.join("portfolio/decisions")).unwrap();

    let state = ApiState::new(
        r#"[{"Name":"Cash","AssetClass":"Cash","Amount":1}]"#.to_string(),
        None,
        None,
        "EUR".to_string(),
    )
    .await
    .unwrap();
    *state.inner.workspace_dir.write().await = Some(workspace.to_string_lossy().to_string());

    // First write succeeds.
    state
        .inner
        .draft_decision(Some("Rebalance".to_string()), None, false)
        .await
        .unwrap();

    // Second call with the same title on the same day must refuse.
    let err = state
        .inner
        .draft_decision(Some("Rebalance".to_string()), None, false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

#[tokio::test]
async fn test_draft_decision_dry_run_previews_existing_file_without_erroring() {
    // Regression test: a dry-run must only ever preview, never error, even
    // when a real decision record for the same title/day already exists.
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(workspace.join("portfolio/decisions")).unwrap();

    let state = ApiState::new(
        r#"[{"Name":"Cash","AssetClass":"Cash","Amount":1}]"#.to_string(),
        None,
        None,
        "EUR".to_string(),
    )
    .await
    .unwrap();
    *state.inner.workspace_dir.write().await = Some(workspace.to_string_lossy().to_string());

    state
        .inner
        .draft_decision(Some("Rebalance".to_string()), None, false)
        .await
        .unwrap();

    let doc = state
        .inner
        .draft_decision(Some("Rebalance".to_string()), None, true)
        .await
        .unwrap();
    assert!(doc.dry_run);
}

#[tokio::test]
async fn test_generate_report_refuses_overwrite() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(workspace.join("portfolio/reports")).unwrap();

    let state = ApiState::new(
        r#"[{"Name":"Cash","AssetClass":"Cash","Amount":1}]"#.to_string(),
        None,
        None,
        "EUR".to_string(),
    )
    .await
    .unwrap();
    *state.inner.workspace_dir.write().await = Some(workspace.to_string_lossy().to_string());

    state.inner.generate_report(None, false).await.unwrap();

    // Same-day regeneration must refuse, not clobber.
    let err = state.inner.generate_report(None, false).await.unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

#[tokio::test]
async fn test_dry_run_preview_does_not_block_writes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(workspace.join("portfolio/reports")).unwrap();

    let state = ApiState::new(
        r#"[{"Name":"Cash","AssetClass":"Cash","Amount":1}]"#.to_string(),
        None,
        None,
        "EUR".to_string(),
    )
    .await
    .unwrap();
    *state.inner.workspace_dir.write().await = Some(workspace.to_string_lossy().to_string());

    // Dry-run previews nothing is written, so a subsequent real write
    // must not trip the overwrite guard.
    state.inner.generate_report(None, true).await.unwrap();
    state.inner.generate_report(None, false).await.unwrap();
}

fn get_with_host(uri: &str, host: &str, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(uri).header("host", host);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(Body::empty()).unwrap()
}

async fn guarded(token: Option<&str>) -> axum::Router {
    let state = ApiState::new(CASH_ONLY.to_string(), None, None, "EUR".to_string())
        .await
        .unwrap();
    create_guarded_router(state, ApiGuard::new(3000, token.map(String::from)))
}

#[tokio::test]
async fn test_host_allowlist() {
    let app = guarded(None).await;
    for host in [
        "localhost:3000",
        "127.0.0.1:3000",
        "[::1]:3000",
        "LOCALHOST:3000",
    ] {
        let r = app
            .clone()
            .oneshot(get_with_host("/api/portfolio", host, None));
        assert_eq!(r.await.unwrap().status(), StatusCode::OK, "{host}");
    }
    for host in [
        "evil.example:3000",
        "localhost:3001",
        "localhost",
        "127.0.0.1",
    ] {
        let r = app.clone().oneshot(get_with_host("/health", host, None));
        assert_eq!(
            r.await.unwrap().status(),
            StatusCode::MISDIRECTED_REQUEST,
            "{host}"
        );
    }
    // No Host header at all.
    let r = app.oneshot(get("/api/portfolio")).await.unwrap();
    assert_eq!(r.status(), StatusCode::MISDIRECTED_REQUEST);
}

#[tokio::test]
async fn test_bearer_token() {
    let app = guarded(Some("fake-test-token")).await;
    let host = "127.0.0.1:3000";
    let cases = [
        ("/api/portfolio", None, StatusCode::UNAUTHORIZED),
        ("/api/portfolio", Some("wrong"), StatusCode::UNAUTHORIZED),
        ("/api/portfolio", Some("fake-test-token"), StatusCode::OK),
        ("/health", None, StatusCode::OK),
    ];
    for (uri, token, want) in cases {
        let r = app.clone().oneshot(get_with_host(uri, host, token));
        assert_eq!(r.await.unwrap().status(), want, "{uri} {token:?}");
    }
    let r = app.oneshot(get_with_host(
        "/api/portfolio",
        "evil:3000",
        Some("fake-test-token"),
    ));
    assert_eq!(r.await.unwrap().status(), StatusCode::MISDIRECTED_REQUEST);
}

#[cfg(unix)]
#[test]
fn test_token_file_is_private_and_replaced() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("api-token");
    std::fs::write(&path, "old").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let token = write_token_file(&path).unwrap();
    assert_eq!(token.len(), 64);
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(on_disk.trim(), token);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_ne!(write_token_file(&path).unwrap(), token);
}

#[tokio::test]
async fn test_positions_by_stable_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    let file = tmp.path().join("positions.json");
    let two = r#"[{"Name": "Cash", "AssetClass": "Cash", "Amount": 1000.0},
                  {"Id": "pos_fixed", "Name": "Savings", "AssetClass": "Cash", "Amount": 5.0}]"#;
    std::fs::write(&file, two).unwrap();
    let app = router_with(two, Some(file.to_str().unwrap().to_string()), None).await;

    let list = body_json(app.clone().oneshot(get("/api/positions")).await.unwrap()).await;
    let first_id = list[0]["positionId"].as_str().unwrap().to_string();
    assert!(first_id.starts_with("pos_"));
    assert_eq!(list[1]["positionId"], "pos_fixed");

    let r = app
        .clone()
        .oneshot(get("/api/positions/pos_fixed"))
        .await
        .unwrap();
    assert_eq!(body_json(r).await["name"], "Savings");

    // Delete the first one; pos_fixed still resolves although its index moved.
    let del = Request::builder()
        .method("DELETE")
        .uri(format!("/api/positions/{first_id}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(del).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    let r = app
        .clone()
        .oneshot(json_request(
            "PUT",
            "/api/positions/pos_fixed",
            serde_json::json!({"amount": 7.0}),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body = body_json(r).await;
    assert_eq!(body["amount"], 7.0);
    assert_eq!(body["id"], 0);
    assert!(std::fs::read_to_string(&file)
        .unwrap()
        .contains("pos_fixed"));

    let r = app
        .clone()
        .oneshot(get("/api/positions/pos_missing"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = app.oneshot(get("/api/positions/not-an-id")).await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}

/// Finance retest A1: /api/context reads the data file's quote cache
/// (offline provider, no network) and never writes it.
#[tokio::test]
async fn test_context_reads_quote_cache_read_only() {
    use portfolio_rs::services::quotes::AnyProvider;
    let tmp = tempfile::TempDir::new().unwrap();
    let file = tmp.path().join("positions.json");
    let positions = r#"[
      {"Name": "Cash", "AssetClass": "Cash", "Amount": 1000.0},
      {"Name": "S&P 500", "AssetClass": "Stocks", "Ticker": "SPY", "Amount": 2,
       "Purchases": [{"Date": "2025/04/15", "Quantity": 2, "Price": 400}]}
    ]"#;
    std::fs::write(&file, positions).unwrap();
    // Five days old: within the cache max-age (7) but past --stale-after (3).
    let now = (chrono::Utc::now() - chrono::Duration::days(5)).to_rfc3339();
    let cache = tmp.path().join(".positions.json.quotes-cache.json");
    let cache_json = serde_json::json!({
        "version": 1,
        "quotes": {"SPY": {"price": 500.0, "currency": "USD", "asOf": now, "fetchedAt": now}},
        "fx": {}
    })
    .to_string();
    std::fs::write(&cache, &cache_json).unwrap();
    let state = ApiState::with_quotes(
        positions.to_string(),
        Some(file.to_string_lossy().into_owned()),
        None,
        "USD".to_string(),
        AnyProvider::Offline,
    )
    .await
    .unwrap();
    let app = create_router(state.clone());
    let response = app.oneshot(get("/api/context")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["summary"]["totalValue"], 2000.0, "{json}");
    // A cached price older than the stale threshold is stale (Answer 3).
    assert_eq!(json["summary"]["dataStatus"], "stale", "{json}");
    assert_eq!(json["positions"][1]["price"], 500.0);
    // A recorded purchase Price needs no historic lookup (retest B7).
    assert_eq!(json["positions"][1]["averageCost"], 400.0, "{json}");
    assert!(!json["dataQualityFlags"]
        .to_string()
        .contains("Missing cost basis"));
    // Gap: /api/portfolio carries the stale flag too.
    let r = create_router(state.clone())
        .oneshot(get("/api/portfolio"))
        .await
        .unwrap();
    let p = body_json(r).await;
    assert_eq!(p["hasStale"], true, "{p}");
    assert_eq!(p["dataStatus"], "stale", "{p}");
    assert_eq!(std::fs::read_to_string(&cache).unwrap(), cache_json);
    let names: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
    assert_eq!(names.len(), 2, "no extra files written");
}
