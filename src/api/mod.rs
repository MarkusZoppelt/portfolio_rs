//! Local HTTP API server backed by Axum.
//!
//! Exposes portfolio data over JSON endpoints for scripts, agents, and
//! external clients on the same machine.
//!
//! # Security
//!
//! The server is intended for local use only. [`run_server`] always applies
//! [`ApiGuard`]:
//!
//! - **Host allowlist:** requests must carry `Host: localhost:<port>`,
//!   `127.0.0.1:<port>` or `[::1]:<port>` (blocks DNS rebinding). Anything
//!   else gets `421 Misdirected Request`.
//! - **Optional bearer token:** with `--token-file`, a random token is written
//!   to that file (mode 0600, never logged) and every `/api/*` request needs
//!   `Authorization: Bearer <token>` (`401` otherwise). `/health` stays open.
//!
//! [`run_server`] warns loudly when binding to a non-loopback address.
//!
//! # Persistence
//!
//! Position mutations (`POST`/`PUT`/`DELETE`) are persisted back to the
//! portfolio file, except for `.gpg` files, which are never rewritten:
//! changes to encrypted portfolios stay in memory for the server's lifetime.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use eyre::{Result, WrapErr};
use serde::Deserialize;
use tokio::net::TcpListener;

use crate::context::PortfolioContext;
use crate::doctor::WorkspaceHealth;
use crate::review::Review;
use crate::services::quote_cache::{cache_path_for, CachedProvider, DEFAULT_MAX_AGE_DAYS};
use crate::services::quotes::AnyProvider;
use crate::simulate::RebalanceSimulation;
use crate::state::{
    AllocationItemDto, AppState, CreatePositionRequest, PerformanceDto, PortfolioSummaryDto,
    PositionDto, PositionKey, UpdatePositionRequest,
};
use crate::validate::ValidationReport;

/// Configuration for [`run_server`].
pub struct ServerConfig {
    /// Host to bind to. Anything other than loopback triggers a warning.
    pub host: String,
    /// Port to bind to.
    pub port: u16,
    /// Raw positions JSON to serve.
    pub positions_json: String,
    /// Backing portfolio file for persistence, if any.
    pub file_path: Option<String>,
    /// Optional policy file enabling `/api/review` and `/api/simulate`.
    pub policy_file: Option<String>,
    /// Display currency.
    pub currency: String,
    /// When set, generate a bearer token into this file (mode 0600) and
    /// require it on every `/api/*` request.
    pub token_file: Option<std::path::PathBuf>,
}

/// Shared handler state.
#[derive(Clone)]
pub struct ApiState {
    pub inner: Arc<AppState>,
}

impl ApiState {
    /// Build the state and perform the initial quote fetch so the server
    /// never responds with an empty portfolio while still starting up.
    pub async fn new(
        positions_json: String,
        file_path: Option<String>,
        policy_file: Option<String>,
        currency: String,
    ) -> Result<Self> {
        Self::with_quotes(
            positions_json,
            file_path,
            policy_file,
            currency,
            AnyProvider::configured().clone(),
        )
        .await
    }

    /// Like [`ApiState::new`] with an explicit quote provider. With a data
    /// file the provider reads that file's quote cache (as written by
    /// `portfolio_rs mcp`), read-only: the API never writes the cache.
    pub async fn with_quotes(
        positions_json: String,
        file_path: Option<String>,
        policy_file: Option<String>,
        currency: String,
        quotes: AnyProvider,
    ) -> Result<Self> {
        let quotes = read_only_cache(quotes, file_path.as_deref());
        let inner = Arc::new(AppState::with_quotes(currency, quotes));
        *inner.file_path.write().await = file_path;
        if let Some(policy) = policy_file {
            inner
                .load_policy(Some(policy.clone()))
                .await
                .wrap_err_with(|| format!("failed to load policy file: {policy}"))?;
        }
        inner.set_positions_json(positions_json).await;
        Ok(Self { inner })
    }
}

/// Wrap `quotes` in a read-only view of the data file's quote cache (never
/// next to an encrypted file, never around a fixture).
fn read_only_cache(quotes: AnyProvider, file_path: Option<&str>) -> AnyProvider {
    let path = file_path
        .filter(|f| !f.ends_with(".gpg"))
        .filter(|_| !matches!(quotes, AnyProvider::Fixture(_) | AnyProvider::Cached(_)))
        .and_then(|f| cache_path_for(std::path::Path::new(f)));
    match path {
        Some(path) => AnyProvider::Cached(Arc::new(CachedProvider::new(
            quotes,
            Some(path),
            DEFAULT_MAX_AGE_DAYS,
            false,
        ))),
        None => quotes,
    }
}

/// JSON error response used by every endpoint.
#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

/// Map an [`AppState`] position error: "not found" becomes 404, everything
/// else is an internal error.
fn map_position_error(e: eyre::Report) -> ApiError {
    if e.to_string().contains("position not found") {
        ApiError::NotFound("position not found".to_string())
    } else {
        ApiError::Internal(format!("{e:#}"))
    }
}

/// `{id}` is a stable `pos_…` id, or a legacy array index.
fn position_key(raw: &str) -> Result<PositionKey, ApiError> {
    PositionKey::parse(raw).ok_or_else(|| {
        ApiError::BadRequest("position id must be a pos_ id or an index".to_string())
    })
}

fn bad_request(e: eyre::Report) -> ApiError {
    ApiError::BadRequest(format!("{e:#}"))
}

fn internal(e: eyre::Report) -> ApiError {
    ApiError::Internal(format!("{e:#}"))
}

/// Build the API router around an initialized [`ApiState`].
pub fn create_router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/portfolio", get(get_portfolio))
        .route("/api/positions", get(get_positions).post(create_position))
        .route(
            "/api/positions/{id}",
            get(get_position)
                .put(update_position)
                .delete(delete_position),
        )
        .route("/api/allocation", get(get_allocation))
        .route("/api/performance", get(get_performance))
        .route("/api/context", get(get_context))
        .route("/api/review", get(get_review))
        .route("/api/simulate", get(get_simulation))
        .route("/api/validate", get(get_validation))
        .route("/api/doctor", get(get_doctor))
        .route("/api/refresh", post(refresh_prices))
        .with_state(state)
}

/// Request guard applied by [`run_server`]: Host allowlist plus optional
/// bearer token. See the module docs.
#[derive(Clone, Debug)]
pub struct ApiGuard {
    allowed_hosts: Vec<String>,
    token: Option<String>,
}

impl ApiGuard {
    /// Allow only `localhost`, `127.0.0.1` and `[::1]` on `port`.
    pub fn new(port: u16, token: Option<String>) -> Self {
        let allowed_hosts = ["localhost", "127.0.0.1", "[::1]"]
            .iter()
            .map(|h| format!("{h}:{port}"))
            .collect();
        Self {
            allowed_hosts,
            token,
        }
    }

    fn host_allowed(&self, host: Option<&str>) -> bool {
        host.is_some_and(|h| {
            self.allowed_hosts
                .iter()
                .any(|a| a.eq_ignore_ascii_case(h.trim()))
        })
    }

    fn token_ok(&self, authorization: Option<&str>) -> bool {
        let Some(expected) = &self.token else {
            return true;
        };
        let Some(given) = authorization.and_then(|v| v.strip_prefix("Bearer ")) else {
            return false;
        };
        constant_time_eq(given.trim().as_bytes(), expected.as_bytes())
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn guard_middleware(
    State(guard): State<Arc<ApiGuard>>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let header_str = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok());
    let host_ok = guard.host_allowed(header_str(header::HOST));
    let token_ok =
        request.uri().path() == "/health" || guard.token_ok(header_str(header::AUTHORIZATION));
    if !host_ok {
        return (
            StatusCode::MISDIRECTED_REQUEST,
            Json(serde_json::json!({ "error": "host not allowed" })),
        )
            .into_response();
    }
    if !token_ok {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            Json(serde_json::json!({ "error": "missing or invalid bearer token" })),
        )
            .into_response();
    }
    next.run(request).await
}

/// [`create_router`] with an [`ApiGuard`] in front of every route.
pub fn create_guarded_router(state: ApiState, guard: ApiGuard) -> Router {
    create_router(state).layer(middleware::from_fn_with_state(
        Arc::new(guard),
        guard_middleware,
    ))
}

/// Generate a random bearer token and write it to `path` with mode 0600,
/// replacing any previous token file. The token is never printed.
pub fn write_token_file(path: &std::path::Path) -> Result<String> {
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).wrap_err("failed to replace token file"),
    }
    crate::services::atomic_write::write_new_private(path, format!("{token}\n").as_bytes())
        .wrap_err_with(|| format!("failed to write token file {}", path.display()))?;
    Ok(token)
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// Start the HTTP API server and serve until interrupted.
pub async fn run_server(config: ServerConfig) -> Result<()> {
    serve_until(
        config,
        AnyProvider::configured().clone(),
        Box::new(serve_forever),
    )
    .await
}

/// A future that, once complete, stops the server gracefully.
type Shutdown = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// Builds the [`Shutdown`] future, given the bound address.
type ShutdownFor = Box<dyn FnOnce(std::net::SocketAddr) -> Shutdown + Send>;

/// [`run_server`]'s shutdown: never, the server runs until interrupted.
fn serve_forever(_bound: std::net::SocketAddr) -> Shutdown {
    Box::pin(std::future::pending())
}

/// [`run_server`] with an explicit quote provider, serving until the
/// future `shutdown` returns for the bound address completes.
///
/// The Host allowlist uses the bound port, so port 0 (any free port) works.
async fn serve_until(
    config: ServerConfig,
    quotes: AnyProvider,
    shutdown: ShutdownFor,
) -> Result<()> {
    let addr = format!("{}:{}", config.host, config.port);

    if !is_loopback_host(&config.host) {
        eprintln!(
            "WARNING: binding to non-loopback address {} — the API exposes your \
             full portfolio (including write access), and the Host allowlist \
             only accepts localhost names. Only do this on a trusted network.",
            config.host
        );
    }

    let token = match &config.token_file {
        Some(path) => {
            let token = write_token_file(path)?;
            println!("API bearer token written to {}", path.display());
            Some(token)
        }
        None => None,
    };

    let state = ApiState::with_quotes(
        config.positions_json,
        config.file_path,
        config.policy_file,
        config.currency,
        quotes,
    )
    .await?;

    let listener = TcpListener::bind(&addr)
        .await
        .wrap_err_with(|| format!("failed to bind to {addr}"))?;
    let local = listener
        .local_addr()
        .wrap_err("failed to read the bound address")?;
    let router = create_guarded_router(state, ApiGuard::new(local.port(), token));
    println!("API server running on http://{addr}");
    println!("Endpoints: http://{addr}/api");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown(local))
        .await
        .wrap_err("API server error")?;
    Ok(())
}

/// Persist after a successful mutation, unless the portfolio is `.gpg`-backed
/// (which is never rewritten) or purely in-memory.
async fn persist(state: &ApiState) -> Result<(), ApiError> {
    if state.inner.can_save().await {
        state.inner.save().await.map_err(internal)?;
    }
    Ok(())
}

async fn get_portfolio(State(state): State<ApiState>) -> Json<PortfolioSummaryDto> {
    Json(state.inner.get_portfolio_summary().await)
}

async fn get_positions(State(state): State<ApiState>) -> Json<Vec<PositionDto>> {
    Json(state.inner.get_positions().await)
}

async fn get_position(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<PositionDto>, ApiError> {
    state
        .inner
        .get_position_by_key(&position_key(&id)?)
        .await
        .map(Json)
        .map_err(map_position_error)
}

async fn get_allocation(State(state): State<ApiState>) -> Json<Vec<AllocationItemDto>> {
    Json(state.inner.get_allocation().await)
}

async fn get_performance(State(state): State<ApiState>) -> Json<PerformanceDto> {
    Json(state.inner.get_performance().await)
}

async fn get_context(State(state): State<ApiState>) -> Result<Json<PortfolioContext>, ApiError> {
    state.inner.get_context().await.map(Json).map_err(internal)
}

async fn get_review(State(state): State<ApiState>) -> Result<Json<Review>, ApiError> {
    state
        .inner
        .get_review()
        .await
        .map(Json)
        .map_err(bad_request)
}

async fn get_simulation(
    State(state): State<ApiState>,
) -> Result<Json<RebalanceSimulation>, ApiError> {
    state
        .inner
        .get_simulation()
        .await
        .map(Json)
        .map_err(bad_request)
}

async fn get_validation(State(state): State<ApiState>) -> Result<Json<ValidationReport>, ApiError> {
    state
        .inner
        .validate_portfolio()
        .await
        .map(Json)
        .map_err(bad_request)
}

#[derive(Deserialize)]
struct DoctorParams {
    dir: Option<String>,
}

async fn get_doctor(
    State(state): State<ApiState>,
    Query(params): Query<DoctorParams>,
) -> Result<Json<WorkspaceHealth>, ApiError> {
    state
        .inner
        .run_doctor(params.dir)
        .await
        .map(Json)
        .map_err(bad_request)
}

async fn refresh_prices(State(state): State<ApiState>) -> Json<PortfolioSummaryDto> {
    state.inner.refresh_prices().await;
    Json(state.inner.get_portfolio_summary().await)
}

async fn create_position(
    State(state): State<ApiState>,
    Json(req): Json<CreatePositionRequest>,
) -> Result<(StatusCode, Json<PositionDto>), ApiError> {
    let dto = state.inner.add_position(req).await.map_err(internal)?;
    persist(&state).await?;
    Ok((StatusCode::CREATED, Json(dto)))
}

async fn update_position(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<UpdatePositionRequest>,
) -> Result<Json<PositionDto>, ApiError> {
    let dto = state
        .inner
        .update_position_by_key(&position_key(&id)?, req)
        .await
        .map_err(map_position_error)?;
    persist(&state).await?;
    Ok(Json(dto))
}

async fn delete_position(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .inner
        .delete_position_by_key(&position_key(&id)?)
        .await
        .map_err(map_position_error)?;
    persist(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const CASH_ONLY: &str = r#"[{"Name": "Cash", "AssetClass": "Cash", "Amount": 1000.0}]"#;

    fn config(host: &str, port: u16, token_file: Option<std::path::PathBuf>) -> ServerConfig {
        ServerConfig {
            host: host.to_string(),
            port,
            positions_json: CASH_ONLY.to_string(),
            file_path: None,
            policy_file: None,
            currency: "EUR".to_string(),
            token_file,
        }
    }

    /// One HTTP/1.1 request over a fresh connection; the raw response.
    async fn http(addr: std::net::SocketAddr, host: &str, path: &str, token: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\n\
             Connection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn test_serves_on_a_free_port_until_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        let token_path = tmp.path().join("token");
        let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve_until(
            config("127.0.0.1", 0, Some(token_path.clone())),
            AnyProvider::Offline,
            Box::new(move |addr| {
                addr_tx.send(addr).unwrap();
                Box::pin(async move {
                    let _ = stop_rx.await;
                })
            }),
        ));
        let addr = addr_rx.await.unwrap();
        assert_ne!(addr.port(), 0);
        let token = std::fs::read_to_string(&token_path).unwrap();
        let token = token.trim();
        let host = format!("127.0.0.1:{}", addr.port());

        let health = http(addr, &host, "/health", "").await;
        assert!(health.starts_with("HTTP/1.1 200"), "{health}");
        assert!(health.ends_with("ok"), "{health}");

        let summary = http(addr, &host, "/api/portfolio", token).await;
        assert!(summary.starts_with("HTTP/1.1 200"), "{summary}");
        assert!(summary.contains("\"totalValue\":1000.0"), "{summary}");

        let no_token = http(addr, &host, "/api/portfolio", "wrong").await;
        assert!(no_token.starts_with("HTTP/1.1 401"), "{no_token}");
        // The allowlist uses the bound port, not the configured 0.
        let wrong_port = http(addr, "127.0.0.1:0", "/health", "").await;
        assert!(wrong_port.starts_with("HTTP/1.1 421"), "{wrong_port}");

        stop_tx.send(()).unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_busy_port_fails_to_bind() {
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = busy.local_addr().unwrap().port();
        let err = serve_until(
            config("127.0.0.1", port, None),
            AnyProvider::Offline,
            Box::new(serve_forever),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("failed to bind to 127.0.0.1:{port}")
        );
    }

    #[tokio::test]
    async fn test_non_loopback_host_with_unwritable_token_file_fails_before_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let token = tmp.path().join("missing").join("token");
        // Warns about the non-loopback host, then stops at the token file.
        let err = serve_until(
            config("0.0.0.0", 0, Some(token.clone())),
            AnyProvider::Offline,
            Box::new(serve_forever),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("failed to write token file"),
            "{err}"
        );
        assert!(!token.exists());
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(["127.0.0.1", "localhost", "::1", "[::1]"]
            .into_iter()
            .all(is_loopback_host));
    }

    #[test]
    fn test_production_shutdown_never_fires() {
        let mut shutdown = serve_forever(([127, 0, 0, 1], 3000).into());
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        for _ in 0..3 {
            assert!(shutdown.as_mut().poll(&mut cx).is_pending());
        }
    }

    #[tokio::test]
    async fn test_unreadable_policy_fails_startup() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = tmp.path().join("missing.toml");
        let policy = policy.to_str().unwrap().to_string();
        let mut cfg = config("127.0.0.1", 0, None);
        cfg.policy_file = Some(policy.clone());
        let err = serve_until(cfg, AnyProvider::Offline, Box::new(serve_forever))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("failed to load policy file: {policy}")
        );
    }
}
