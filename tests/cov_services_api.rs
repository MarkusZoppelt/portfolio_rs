//! HTTP API endpoints and `portfolio_rs api` paths not covered by
//! `api_offline.rs`. Cash-only positions and offline quotes: no network.

use assert_cmd::Command;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use predicates::str;
use tempfile::TempDir;
use tower::ServiceExt;

use portfolio_rs::api::{create_router, write_token_file, ApiState};

const CASH_ONLY: &str = r#"[{"Name": "Cash", "AssetClass": "Cash", "Amount": 1000.0}]"#;

async fn router(positions_json: &str, file_path: Option<String>) -> axum::Router {
    let state = ApiState::new(
        positions_json.to_string(),
        file_path,
        None,
        "EUR".to_string(),
    )
    .await
    .unwrap();
    create_router(state)
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string())),
        None => builder.body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn test_allocation_performance_and_refresh() {
    let app = router(CASH_ONLY, None).await;

    let (status, allocation) = send(&app, "GET", "/api/allocation", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(allocation[0]["assetClass"], "Cash");
    assert_eq!(allocation[0]["value"], 1000.0);
    assert_eq!(allocation[0]["percent"], 100.0);

    let (status, performance) = send(&app, "GET", "/api/performance", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(performance["totalValue"], 1000.0);
    assert_eq!(performance["cashValue"], 1000.0);
    assert_eq!(performance["securitiesValue"], 0.0);

    let (status, summary) = send(&app, "POST", "/api/refresh", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["totalValue"], 1000.0);
    assert_eq!(summary["positions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_doctor_with_and_without_a_directory() {
    let app = router(CASH_ONLY, None).await;
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    let (status, health) = send(&app, "GET", &format!("/api/doctor?dir={dir}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["dir"], dir);
    assert!(!health["checks"].as_array().unwrap().is_empty());

    let (status, error) = send(&app, "GET", "/api/doctor", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error["error"]
        .as_str()
        .unwrap()
        .contains("no workspace directory"));
}

#[tokio::test]
async fn test_unparseable_positions_make_mutations_internal_errors() {
    let app = router("not json", None).await;
    let body = serde_json::json!({ "amount": 1.0 });
    let (status, error) = send(&app, "PUT", "/api/positions/0", Some(body)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(error["error"]
        .as_str()
        .unwrap()
        .contains("failed to parse current positions"));

    let (status, error) = send(&app, "DELETE", "/api/positions/0", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(error["error"]
        .as_str()
        .unwrap()
        .contains("failed to parse current positions"));
}

#[tokio::test]
async fn test_failed_save_is_an_internal_error() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("missing").join("positions.json");
    let app = router(CASH_ONLY, Some(file.to_str().unwrap().to_string())).await;
    let body = serde_json::json!({
        "name": "Savings", "assetClass": "Cash", "amount": 500.0, "purchases": []
    });
    let (status, error) = send(&app, "POST", "/api/positions", Some(body)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(error["error"]
        .as_str()
        .unwrap()
        .contains("failed to write portfolio file"));
    assert!(!file.exists());
}

#[test]
fn test_token_file_new_and_unreplaceable() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("token");
    let token = write_token_file(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{token}\n")
    );

    // A directory in the way cannot be removed as a file.
    let dir = tmp.path().join("dir");
    std::fs::create_dir(&dir).unwrap();
    let err = write_token_file(&dir).unwrap_err();
    assert!(
        err.to_string().contains("failed to replace token file"),
        "{err}"
    );
    assert!(dir.is_dir());
}

/// `portfolio_rs api` loads the file and fails cleanly when the port is taken.
#[test]
fn test_api_command_reports_a_busy_port() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let file = data.path().join("positions.json");
    std::fs::write(&file, CASH_ONLY).unwrap();
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = busy.local_addr().unwrap().port().to_string();

    Command::cargo_bin("portfolio_rs")
        .unwrap()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("XDG_DATA_HOME", home.path().join(".local/share"))
        .env("PORTFOLIO_RS_QUOTES", "offline")
        .current_dir(data.path())
        .args(["api", "--host", "127.0.0.1", "--port", &port])
        .arg(&file)
        .assert()
        .failure()
        .stderr(str::contains(format!("failed to bind to 127.0.0.1:{port}")));
    drop(busy);
}
