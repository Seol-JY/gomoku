//! Router assembly; shared with the integration tests

#![forbid(unsafe_code)]

pub mod auth;
pub mod db;
pub mod error;
pub mod ids;
pub mod room;
pub mod routes;
pub mod state;
pub mod time;

use axum::Router;
use axum::http::header::{CACHE_CONTROL, HeaderName, HeaderValue};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

pub use state::{AppState, Config};

pub fn build_app(state: AppState) -> Router {
    routes::router(state)
        // proxy buffering off for SSE
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .layer(TraceLayer::new_for_http())
}
