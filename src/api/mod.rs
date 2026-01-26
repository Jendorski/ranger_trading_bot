pub mod handlers;

use axum::{routing::get, Router};
use redis::aio::MultiplexedConnection;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};

/// Shared state for API handlers
#[derive(Clone)]
pub struct ApiState {
    pub redis_conn: Arc<Mutex<MultiplexedConnection>>,
}

/// Create and configure the API router
pub fn create_router(redis_conn: MultiplexedConnection) -> Router {
    let state = ApiState {
        redis_conn: Arc::new(Mutex::new(redis_conn)),
    };

    // Configure CORS to allow all origins (adjust for production)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/positions/closed", get(handlers::get_closed_positions))
        .route("/api/positions/active", get(handlers::get_active_position))
        .route(
            "/api/positions/profit-targets",
            get(handlers::get_profit_targets),
        )
        .route(
            "/api/capitulation/closed",
            get(handlers::get_capitulation_closed_positions),
        )
        .route(
            "/api/capitulation/state",
            get(handlers::get_capitulation_state),
        )
        .route(
            "/api/capitulation/capital",
            axum::routing::post(handlers::update_capitulation_capital),
        )
        .route("/api/capital", get(handlers::get_trading_capital))
        .route("/api/analytics/weekly", get(handlers::get_weekly_roi))
        .route("/api/analytics/monthly", get(handlers::get_monthly_roi))
        .layer(cors)
        .with_state(state)
}
