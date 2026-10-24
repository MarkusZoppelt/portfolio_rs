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
        let inner = Arc::new(AppState::new(currency));
        *inner.file_path.write().await = file_path;
        if let Some(policy) = policy_file {
            inner
                .load_policy(Some(policy.clone()))
                .await
                .wrap_err_with(|| format!("failed to load policy file: {}", policy))?;
        }
        inner.set_positions_json(positions_json).await;
        Ok(Self { inner })
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
        ApiError::Internal(format!("{:#}", e))
    }
}

/// `{id}` is a stable `pos_…` id, or a legacy array index.
fn position_key(raw: &str) -> Result<PositionKey, ApiError> {
    PositionKey::parse(raw).ok_or_else(|| {
        ApiError::BadRequest("position id must be a pos_ id or an index".to_string())
    })
}

fn bad_request(e: eyre::Report) -> ApiError {
    ApiError::BadRequest(format!("{:#}", e))
}

fn internal(e: eyre::Report) -> ApiError {
    ApiError::Internal(format!("{:#}", e))
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
            .map(|h| format!("{}:{}", h, port))
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
    crate::services::atomic_write::write_new_private(path, format!("{}\n", token).as_bytes())
        .wrap_err_with(|| format!("failed to write token file {}", path.display()))?;
    Ok(token)
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// Start the HTTP API server and serve until interrupted.
pub async fn run_server(config: ServerConfig) -> Result<()> {
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
    let guard = ApiGuard::new(config.port, token);

    let state = ApiState::new(
        config.positions_json,
        config.file_path,
        config.policy_file,
        config.currency,
    )
    .await?;
    let router = create_guarded_router(state, guard);

    let listener = TcpListener::bind(&addr)
        .await
        .wrap_err_with(|| format!("failed to bind to {}", addr))?;
    println!("API server running on http://{}", addr);
    println!("Endpoints: http://{}/api", addr);

    axum::serve(listener, router)
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
