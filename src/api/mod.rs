//! Local HTTP API server backed by Axum.
//!
//! Exposes portfolio data over JSON endpoints for scripts, agents, and
//! external clients on the same machine.
//!
//! # Security
//!
//! The server has **no authentication** and is intended for local use only.
//! [`run_server`] warns loudly when binding to a non-loopback address.
//!
//! # Persistence
//!
//! Position mutations (`POST`/`PUT`/`DELETE`) are persisted back to the
//! portfolio file, except for `.gpg` files, which are never rewritten:
//! changes to encrypted portfolios stay in memory for the server's lifetime.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
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
    PositionDto, UpdatePositionRequest,
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
            "/api/positions/:id",
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

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// Start the HTTP API server and serve until interrupted.
pub async fn run_server(config: ServerConfig) -> Result<()> {
    let addr = format!("{}:{}", config.host, config.port);

    if !is_loopback_host(&config.host) {
        eprintln!(
            "WARNING: binding to non-loopback address {} — the API has no \
             authentication and exposes your full portfolio (including write \
             access). Only do this on a trusted network.",
            config.host
        );
    }

    let state = ApiState::new(
        config.positions_json,
        config.file_path,
        config.policy_file,
        config.currency,
    )
    .await?;
    let router = create_router(state);

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
    Path(id): Path<usize>,
) -> Result<Json<PositionDto>, ApiError> {
    state
        .inner
        .get_position(id)
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
    Path(id): Path<usize>,
    Json(req): Json<UpdatePositionRequest>,
) -> Result<Json<PositionDto>, ApiError> {
    let dto = state
        .inner
        .update_position(id, req)
        .await
        .map_err(map_position_error)?;
    persist(&state).await?;
    Ok(Json(dto))
}

async fn delete_position(
    State(state): State<ApiState>,
    Path(id): Path<usize>,
) -> Result<StatusCode, ApiError> {
    state
        .inner
        .delete_position(id)
        .await
        .map_err(map_position_error)?;
    persist(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}
