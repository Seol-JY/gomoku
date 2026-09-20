//! Bearer tokens; anonymous user = full account without an `identities` row,
//! the token only ties POSTs to the same person's SSE stream

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::error::AppError;
use crate::state::AppState;
use crate::{db, ids, time};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthUser {
    pub user_id: String,
    pub display_name: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(AppError::unauthorized)?;
        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(AppError::unauthorized)?;
        let hash = ids::hash_token(token);
        let now = time::now();
        let user = db::user_by_token_hash(&state.db, &hash, now)
            .await?
            .ok_or_else(AppError::unauthorized)?;
        // best effort: never fail the request
        if let Err(e) = db::touch_credential(&state.db, &hash, now).await {
            tracing::debug!(error = %e, "touch credential failed");
        }
        Ok(Self {
            user_id: user.id,
            display_name: user.display_name,
        })
    }
}
