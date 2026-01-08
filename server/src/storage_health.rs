use axum::{
    extract::State,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Mutex;

use storage::health::SdHealth;
use crate::AppState;

pub struct HealthState {
    pub cached_health: Mutex<Option<SdHealth>>,
}

impl Default for HealthState {
    fn default() -> Self {
        Self {
            cached_health: Mutex::new(None),
        }
    }
}

#[derive(Serialize)]
pub struct HealthStatusResponse {
    pub status: String,
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub wear_level: Option<u8>,
    pub health_score: u8,
    pub temperature: Option<i32>,
    pub io_errors: u32,
    pub last_check: String,
}

pub async fn health_status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let health = get_or_refresh_health(&state.health_state).await;
    Json(HealthStatusResponse {
        status: health.status.to_string(),
    })
}

pub async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let health = get_or_refresh_health(&state.health_state).await;

    Json(HealthResponse {
        status: health.status.to_string(),
        wear_level: health.wear_level,
        health_score: health.health_score,
        temperature: health.temperature,
        io_errors: health.io_errors,
        last_check: health.last_check.to_rfc3339(),
    })
}

pub async fn detailed_health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let health = get_or_refresh_health(&state.health_state).await;
    Json(health)
}

pub async fn refresh_health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    {
        let mut cached = state.health_state.cached_health.lock().await;
        *cached = None;
    }

    let health = get_or_refresh_health(&state.health_state).await;
    Json(health)
}

async fn get_or_refresh_health(health_state: &Arc<HealthState>) -> SdHealth {
    let health = storage::health::check_health_with_logging().await;

    {
        let mut cached = health_state.cached_health.lock().await;
        *cached = Some(health.clone());
    }

    health
}
