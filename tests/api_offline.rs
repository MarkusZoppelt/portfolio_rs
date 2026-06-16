//! Offline tests for the HTTP API router.
//!
//! Uses cash-only positions (no tickers) so no live quotes are fetched and
//! the tests are fully deterministic without network access.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use portfolio_rs::api::{create_router, ApiState};

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
