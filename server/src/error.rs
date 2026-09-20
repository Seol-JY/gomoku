//! Handler error: `ApiError` + HTTP status

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use proto::{ApiError, ErrorCode};

#[derive(Debug, Clone)]
pub struct AppError(pub ApiError);

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self(ApiError::new(code, message))
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }

    pub fn unauthorized() -> Self {
        Self::new(ErrorCode::Unauthorized, "missing or invalid token")
    }

    pub fn not_found(what: &str) -> Self {
        Self::new(ErrorCode::NotFound, format!("{what} not found"))
    }

    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!(error = %err, "internal error");
        Self::new(ErrorCode::Internal, "internal server error")
    }

    #[must_use]
    pub fn with_forbidden(mut self, kind: proto::ForbiddenKind) -> Self {
        self.0.forbidden = Some(kind);
        self
    }

    #[must_use]
    pub fn with_current_seq(mut self, seq: u64) -> Self {
        self.0.current_seq = Some(seq);
        self
    }

    #[must_use]
    pub fn with_min_client_version(mut self, v: &str) -> Self {
        self.0.min_client_version = Some(v.to_owned());
        self
    }

    pub const fn code(&self) -> ErrorCode {
        self.0.code
    }
}

impl From<ApiError> for AppError {
    fn from(e: ApiError) -> Self {
        Self(e)
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        Self::internal(e)
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(e)
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self.0)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
