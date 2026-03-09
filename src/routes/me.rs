//! Current user profile endpoint.
//!
//! GET /api/me — returns the authenticated user's account details

use axum::{Extension, Json};
use serde::Serialize;
use uuid::Uuid;

use crate::models::{AuthUser, SubscriptionTier};

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
}

pub async fn me(Extension(user): Extension<AuthUser>) -> Json<MeResponse> {
    Json(MeResponse {
        user_id: user.id,
        email: user.email,
        email_verified: user.email_verified,
        tier_display: user.tier.display_name(),
        max_watchers: user.tier.max_watchers(),
        check_interval_minutes: user.tier.check_interval_minutes(),
        monthly_price_usd: user.tier.monthly_price_usd(),
        tier: user.tier,
    })
}
