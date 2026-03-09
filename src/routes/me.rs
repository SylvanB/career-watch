//! Current user profile endpoint.
//!
//! GET /api/me — returns the authenticated user's account details

use axum::{extract::State, http::StatusCode, Extension, Json};
use serde::Serialize;
use uuid::Uuid;

use crate::models::{AuthUser, SubscriptionTier};
use crate::AppState;

#[derive(Serialize)]
pub struct MeResponse {
    pub user_id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub tier: SubscriptionTier,
    pub tier_display: &'static str,
    pub max_watchers: i64,
    pub check_interval_minutes: i64,
    pub monthly_price_usd: Option<u32>,
    pub unread_notifications: i64,
}

pub async fn me(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<MeResponse>, StatusCode> {
    let unread_notifications: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications WHERE user_id = $1 AND read_at IS NULL",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(MeResponse {
        user_id: user.id,
        email: user.email.clone(),
        email_verified: user.email_verified,
        tier_display: user.tier.display_name(),
        max_watchers: user.tier.max_watchers(),
        check_interval_minutes: user.tier.check_interval_minutes(),
        monthly_price_usd: user.tier.monthly_price_usd(),
        tier: user.tier,
        unread_notifications,
    }))
}
