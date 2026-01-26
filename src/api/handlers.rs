use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use log::info;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use super::ApiState;
use crate::bot::capitulation_phase::{self, CapitulationState};
use crate::bot::{ClosedPosition, OpenPosition};
use crate::helper::{
    PartialProfitTarget, CAPITULATION_PHASE_CLOSED_POSITIONS, CAPITULATION_PHASE_STATE,
    TRADING_BOT_ACTIVE, TRADING_BOT_CLOSE_POSITIONS, TRADING_CAPITAL,
    TRADING_PARTIAL_PROFIT_TARGET,
};

/// Pagination query parameters
#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page")]
    pub page: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Optional start date filter (ISO 8601 format: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS)
    pub from_date: Option<String>,
    /// Optional end date filter (ISO 8601 format: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS)
    pub to_date: Option<String>,
}

fn default_page() -> usize {
    1
}

fn default_limit() -> usize {
    20
}

/// Paginated response for closed positions
#[derive(Debug, Serialize)]
pub struct ClosedPositionsResponse {
    pub positions: Vec<ClosedPosition>,
    pub total: usize,
    pub page: usize,
    pub limit: usize,
}

/// Error response structure
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Custom error type that implements IntoResponse
pub enum ApiError {
    RedisError(String),
    NotFound(String),
    InvalidInput(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::RedisError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg),
        };

        let body = Json(ErrorResponse { error: message });
        (status, body).into_response()
    }
}

/// Parse date string (ISO 8601) into DateTime<Utc>
fn parse_date(date_str: &str) -> Result<DateTime<Utc>, ApiError> {
    // Try parsing with time first (YYYY-MM-DDTHH:MM:SS or full RFC3339)
    if let Ok(dt) = DateTime::parse_from_rfc3339(date_str) {
        return Ok(dt.with_timezone(&Utc));
    }

    // Try parsing date only (YYYY-MM-DD) - set to start of day UTC
    if let Ok(naive_date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        if let Some(naive_datetime) = naive_date.and_hms_opt(0, 0, 0) {
            return Ok(DateTime::from_naive_utc_and_offset(naive_datetime, Utc));
        }
    }

    Err(ApiError::InvalidInput(format!(
        "Invalid date format '{}'. Use ISO 8601: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ",
        date_str
    )))
}

/// GET /api/positions/closed
/// Returns paginated list of closed positions with optional date filtering
pub async fn get_closed_positions(
    Query(params): Query<PaginationParams>,
    State(state): State<ApiState>,
) -> Result<Json<ClosedPositionsResponse>, ApiError> {
    // Validate pagination parameters
    if params.page == 0 {
        return Err(ApiError::InvalidInput(
            "Page must be greater than 0".to_string(),
        ));
    }
    if params.limit == 0 || params.limit > 20 {
        return Err(ApiError::InvalidInput(
            "Limit must be between 1 and 20".to_string(),
        ));
    }

    // Parse date filters if provided
    let from_date = params
        .from_date
        .as_ref()
        .map(|s| parse_date(s))
        .transpose()?;
    let to_date = params.to_date.as_ref().map(|s| parse_date(s)).transpose()?;

    let mut conn = state.redis_conn.lock().await;

    // When filtering by date, fetch all positions and filter in-app
    let raw_positions: Vec<String> = if from_date.is_some() || to_date.is_some() {
        conn.lrange(TRADING_BOT_CLOSE_POSITIONS, 0, -1)
            .await
            .map_err(|e| ApiError::RedisError(format!("Failed to fetch positions: {}", e)))?
    } else {
        let start = (params.page - 1) * params.limit;
        let end = start + params.limit - 1;
        conn.lrange(TRADING_BOT_CLOSE_POSITIONS, start as isize, end as isize)
            .await
            .map_err(|e| ApiError::RedisError(format!("Failed to fetch positions: {}", e)))?
    };

    // Deserialize and filter positions
    let mut positions: Vec<ClosedPosition> = raw_positions
        .iter()
        .filter_map(|p| serde_json::from_str(p).ok())
        .filter(|pos: &ClosedPosition| {
            if let Some(from) = from_date {
                if pos.exit_time < from {
                    return false;
                }
            }
            if let Some(to) = to_date {
                if pos.exit_time > to {
                    return false;
                }
            }
            true
        })
        .collect();

    let total_filtered = positions.len();

    // Apply pagination after filtering for date queries
    if from_date.is_some() || to_date.is_some() {
        let start = (params.page - 1) * params.limit;
        positions = positions
            .into_iter()
            .skip(start)
            .take(params.limit)
            .collect();
    }

    Ok(Json(ClosedPositionsResponse {
        positions,
        total: total_filtered,
        page: params.page,
        limit: params.limit,
    }))
}

/// GET /api/positions/active
/// Returns the current active position or null if none
pub async fn get_active_position(
    State(state): State<ApiState>,
) -> Result<Json<Option<OpenPosition>>, ApiError> {
    let mut conn = state.redis_conn.lock().await;

    // Try to fetch the active position
    let raw_position: Option<String> = conn
        .get(TRADING_BOT_ACTIVE)
        .await
        .map_err(|e| ApiError::RedisError(format!("Failed to fetch active position: {}", e)))?;

    match raw_position {
        Some(raw) => {
            let position: OpenPosition = serde_json::from_str(&raw).map_err(|e| {
                ApiError::RedisError(format!("Failed to deserialize position: {}", e))
            })?;
            Ok(Json(Some(position)))
        }
        None => Ok(Json(None)),
    }
}

/// GET /api/positions/profit-targets
/// Returns the current partial profit targets
pub async fn get_profit_targets(
    State(state): State<ApiState>,
) -> Result<Json<Vec<PartialProfitTarget>>, ApiError> {
    let mut conn = state.redis_conn.lock().await;

    // Try to fetch profit targets
    let raw_targets: Option<String> = conn
        .get(TRADING_PARTIAL_PROFIT_TARGET)
        .await
        .map_err(|e| ApiError::RedisError(format!("Failed to fetch profit targets: {}", e)))?;

    match raw_targets {
        Some(raw) => {
            let targets: Vec<PartialProfitTarget> = serde_json::from_str(&raw).map_err(|e| {
                ApiError::RedisError(format!("Failed to deserialize targets: {}", e))
            })?;
            Ok(Json(targets))
        }
        None => Ok(Json(Vec::new())),
    }
}

/// GET /api/capitulation/closed
/// Returns paginated list of capitulation phase closed positions with optional date filtering
pub async fn get_capitulation_closed_positions(
    Query(params): Query<PaginationParams>,
    State(state): State<ApiState>,
) -> Result<Json<ClosedPositionsResponse>, ApiError> {
    // Validate pagination parameters
    if params.page == 0 {
        return Err(ApiError::InvalidInput(
            "Page must be greater than 0".to_string(),
        ));
    }
    if params.limit == 0 || params.limit > 20 {
        return Err(ApiError::InvalidInput(
            "Limit must be between 1 and 20".to_string(),
        ));
    }

    // Parse date filters if provided
    let from_date = params
        .from_date
        .as_ref()
        .map(|s| parse_date(s))
        .transpose()?;
    let to_date = params.to_date.as_ref().map(|s| parse_date(s)).transpose()?;

    let mut conn = state.redis_conn.lock().await;

    // When filtering by date, fetch all positions and filter in-app
    let raw_positions: Vec<String> = if from_date.is_some() || to_date.is_some() {
        conn.lrange(CAPITULATION_PHASE_CLOSED_POSITIONS, 0, -1)
            .await
            .map_err(|e| ApiError::RedisError(format!("Failed to fetch positions: {}", e)))?
    } else {
        let start = (params.page - 1) * params.limit;
        let end = start + params.limit - 1;
        conn.lrange(
            CAPITULATION_PHASE_CLOSED_POSITIONS,
            start as isize,
            end as isize,
        )
        .await
        .map_err(|e| ApiError::RedisError(format!("Failed to fetch positions: {}", e)))?
    };

    // Deserialize and filter positions
    let mut positions: Vec<ClosedPosition> = raw_positions
        .iter()
        .filter_map(|p| serde_json::from_str(p).ok())
        .filter(|pos: &ClosedPosition| {
            if let Some(from) = from_date {
                if pos.exit_time < from {
                    return false;
                }
            }
            if let Some(to) = to_date {
                if pos.exit_time > to {
                    return false;
                }
            }
            true
        })
        .collect();

    let total_filtered = positions.len();

    // Apply pagination after filtering for date queries
    if from_date.is_some() || to_date.is_some() {
        let start = (params.page - 1) * params.limit;
        positions = positions
            .into_iter()
            .skip(start)
            .take(params.limit)
            .collect();
    }

    Ok(Json(ClosedPositionsResponse {
        positions,
        total: total_filtered,
        page: params.page,
        limit: params.limit,
    }))
}

/// GET /api/capitulation/state
/// Returns the current capitulation phase state
pub async fn get_capitulation_state(
    State(state): State<ApiState>,
) -> Result<Json<Option<CapitulationState>>, ApiError> {
    let mut conn = state.redis_conn.lock().await;

    // Try to fetch the capitulation state
    let raw_state: Option<String> = conn
        .get(CAPITULATION_PHASE_STATE)
        .await
        .map_err(|e| ApiError::RedisError(format!("Failed to fetch capitulation state: {}", e)))?;

    match raw_state {
        Some(raw) => {
            let cap_state: CapitulationState = serde_json::from_str(&raw)
                .map_err(|e| ApiError::RedisError(format!("Failed to deserialize state: {}", e)))?;
            Ok(Json(Some(cap_state)))
        }
        None => Ok(Json(None)),
    }
}

/// Request to update capitulation capital
#[derive(Debug, Deserialize)]
pub struct UpdateCapitalRequest {
    pub capital: rust_decimal::Decimal,
}

/// POST /api/capitulation/capital
/// Updates the current capitulation capital
pub async fn update_capitulation_capital(
    State(state): State<ApiState>,
    Json(payload): Json<UpdateCapitalRequest>,
) -> Result<Json<CapitulationState>, ApiError> {
    let mut conn = state.redis_conn.lock().await;

    let old_state = capitulation_phase::CapitulationState::load_state(&mut conn)
        .await
        .unwrap();
    info!("Old state: {:?}", old_state);

    let cap_state = capitulation_phase::CapitulationState::update_capital(
        old_state,
        conn.clone(),
        payload.capital,
    )
    .await
    .map_err(|e| ApiError::RedisError(format!("Failed to update capital: {}", e)))?;

    Ok(Json(cap_state))
}

/// Response for trading capital
#[derive(Debug, Serialize)]
pub struct TradingCapitalResponse {
    pub capital: String,
}

/// GET /api/capital
/// Returns the current trading capital
pub async fn get_trading_capital(
    State(state): State<ApiState>,
) -> Result<Json<TradingCapitalResponse>, ApiError> {
    let mut conn = state.redis_conn.lock().await;

    // Try to fetch the trading capital
    let raw_capital: Option<String> = conn
        .get(TRADING_CAPITAL)
        .await
        .map_err(|e| ApiError::RedisError(format!("Failed to fetch trading capital: {}", e)))?;

    match raw_capital {
        Some(capital) => Ok(Json(TradingCapitalResponse { capital })),
        None => Err(ApiError::NotFound("Trading capital not found".to_string())),
    }
}

/// Response for weekly ROI data
#[derive(Debug, Serialize)]
pub struct WeeklyRoiEntry {
    pub year: i32,
    pub week: u32,
    pub roi_percent: f64,
}

#[derive(Debug, Serialize)]
pub struct WeeklyRoiResponse {
    pub data: Vec<WeeklyRoiEntry>,
}

/// Response for monthly ROI data
#[derive(Debug, Serialize)]
pub struct MonthlyRoiEntry {
    pub year: i32,
    pub month: u32,
    pub roi_percent: f64,
}

#[derive(Debug, Serialize)]
pub struct MonthlyRoiResponse {
    pub data: Vec<MonthlyRoiEntry>,
}

/// GET /api/analytics/weekly
/// Returns weekly ROI breakdown
pub async fn get_weekly_roi(
    State(state): State<ApiState>,
) -> Result<Json<WeeklyRoiResponse>, ApiError> {
    use crate::graph::Graph;

    let mut conn = state.redis_conn.lock().await;

    // Load all closed positions
    let positions = Graph::load_all_closed_positions(&mut conn)
        .await
        .map_err(|e| ApiError::RedisError(format!("Failed to load positions: {}", e)))?;

    // Calculate weekly ROI
    let mut graph = Graph::new();
    let weekly_roi = graph.cumulative_roi_weekly(&positions);

    // Convert to response format and sort by year/week
    let mut data: Vec<WeeklyRoiEntry> = weekly_roi
        .into_iter()
        .map(|((year, week), roi)| WeeklyRoiEntry {
            year,
            week,
            roi_percent: roi,
        })
        .collect();

    data.sort_by_key(|entry| (entry.year, entry.week));

    Ok(Json(WeeklyRoiResponse { data }))
}

/// GET /api/analytics/monthly
/// Returns monthly ROI breakdown
pub async fn get_monthly_roi(
    State(state): State<ApiState>,
) -> Result<Json<MonthlyRoiResponse>, ApiError> {
    use crate::graph::Graph;

    let mut conn = state.redis_conn.lock().await;

    // Load all closed positions
    let positions = Graph::load_all_closed_positions(&mut conn)
        .await
        .map_err(|e| ApiError::RedisError(format!("Failed to load positions: {}", e)))?;

    // Calculate monthly ROI
    let mut graph = Graph::new();
    let monthly_roi = graph.cumulative_roi_monthly(&positions);

    // Convert to response format and sort by year/month
    let mut data: Vec<MonthlyRoiEntry> = monthly_roi
        .into_iter()
        .map(|((year, month), roi)| MonthlyRoiEntry {
            year,
            month,
            roi_percent: roi,
        })
        .collect();

    data.sort_by_key(|entry| (entry.year, entry.month));

    Ok(Json(MonthlyRoiResponse { data }))
}
