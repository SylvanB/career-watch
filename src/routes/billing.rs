//! Billing route handlers.
//!
//! POST /billing/checkout  — create a Stripe Checkout session (upgrade)
//! POST /billing/portal    — create a Stripe Billing Portal session (manage)
//! POST /billing/webhook   — receive Stripe webhook events
//!
//! The webhook route is intentionally NOT behind auth middleware; it uses
//! Stripe's own signature verification instead.

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    Extension, Json,
};
use serde::{Deserialize, Serialize};

use crate::billing::{
    stripe_client::verify_webhook_signature,
    webhooks::handle_event,
};
use crate::models::AuthUser;
use crate::AppState;

// ── Checkout ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CheckoutRequest {
    /// Which plan the user wants to upgrade to: "pro" or "business".
    pub plan: String,
}

#[derive(Serialize)]
pub struct CheckoutResponse {
    pub checkout_url: String,
}

/// Create a Stripe Checkout session and return the hosted URL.
///
/// The user must have a Stripe customer ID (created at registration). If they
/// don't (e.g. Stripe was unavailable at signup), we create one now.
pub async fn create_checkout(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(payload): Json<CheckoutRequest>,
) -> Result<Json<CheckoutResponse>, StatusCode> {
    let price_id = match payload.plan.as_str() {
        "pro" => &state.config.stripe_price_pro,
        "business" => &state.config.stripe_price_business,
        _ => return Err(StatusCode::BAD_REQUEST),
    };

    if price_id.is_empty() {
        tracing::warn!("Stripe price ID not configured for plan '{}'", payload.plan);
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    // Get or create Stripe customer ID
    let customer_id = match user.stripe_customer_id.clone() {
        Some(id) => id,
        None => {
            let id = state
                .stripe
                .create_customer(&user.email, &user.id.to_string())
                .await
                .map_err(|e| {
                    tracing::error!("Stripe customer creation failed: {e}");
                    StatusCode::SERVICE_UNAVAILABLE
                })?;

            let _ = sqlx::query(
                "UPDATE users SET stripe_customer_id = $1 WHERE id = $2",
            )
            .bind(&id)
            .bind(user.id)
            .execute(&state.db)
            .await;

            id
        }
    };

    let success_url = format!("{}/dashboard?upgraded=1", state.config.app_base_url);
    let cancel_url = format!("{}/pricing", state.config.app_base_url);

    let checkout_url = state
        .stripe
        .create_checkout_session(&customer_id, price_id, &success_url, &cancel_url)
        .await
        .map_err(|e| {
            tracing::error!("Stripe Checkout session creation failed: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })?;

    Ok(Json(CheckoutResponse { checkout_url }))
}

// ── Portal ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct PortalResponse {
    pub portal_url: String,
}

/// Create a Stripe Billing Portal session so the user can manage their subscription.
pub async fn create_portal(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<PortalResponse>, StatusCode> {
    let customer_id = user
        .stripe_customer_id
        .ok_or(StatusCode::BAD_REQUEST)?;

    let return_url = format!("{}/dashboard", state.config.app_base_url);

    let portal_url = state
        .stripe
        .create_portal_session(&customer_id, &return_url)
        .await
        .map_err(|e| {
            tracing::error!("Stripe portal session creation failed: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        })?;

    Ok(Json(PortalResponse { portal_url }))
}

// ── Webhook ───────────────────────────────────────────────────────────────────

/// Receive and process Stripe webhook events.
///
/// This handler reads the raw request bytes (required for signature
/// verification) before any JSON parsing.
pub async fn stripe_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // Verify Stripe-Signature header before touching the payload
    let sig = match headers.get("stripe-signature").and_then(|v| v.to_str().ok()) {
        Some(s) => s.to_owned(),
        None => {
            tracing::warn!("webhook received without Stripe-Signature header");
            return StatusCode::BAD_REQUEST;
        }
    };

    if state.config.stripe_webhook_secret.is_empty() {
        tracing::error!("STRIPE_WEBHOOK_SECRET is not configured");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    if !verify_webhook_signature(&body, &sig, &state.config.stripe_webhook_secret) {
        tracing::warn!("webhook signature verification failed");
        return StatusCode::BAD_REQUEST;
    }

    // Parse the event
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("failed to parse webhook body: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    let event_id = event["id"].as_str().unwrap_or("unknown");
    let event_type = event["type"].as_str().unwrap_or("unknown");
    let data = &event["data"];

    match handle_event(
        &state.db,
        &state.email,
        event_id,
        event_type,
        data,
        &state.config.stripe_price_pro,
        &state.config.stripe_price_business,
        &state.config.app_base_url,
    )
    .await
    {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            tracing::error!(event_id, event_type, "webhook processing failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
