//! Stripe webhook event processing.
//!
//! Events are processed idempotently: each `stripe_event_id` is recorded in
//! the `stripe_events` table so re-delivered events are no-ops.

use anyhow::Result;
use serde_json::Value;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::models::SubscriptionTier;

/// Record and process a verified Stripe webhook event.
///
/// Returns `Ok(false)` if the event has already been processed (idempotent
/// skip), `Ok(true)` on successful first processing.
pub async fn handle_event(
    db: &PgPool,
    event_id: &str,
    event_type: &str,
    data: &Value,
    price_pro: &str,
    price_business: &str,
) -> Result<bool> {
    // Idempotency: skip already-processed events.
    let already_seen = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM stripe_events WHERE stripe_event_id = $1)",
    )
    .bind(event_id)
    .fetch_one(db)
    .await?;

    if already_seen {
        info!(event_id, "stripe event already processed, skipping");
        return Ok(false);
    }

    // Record the event before processing so concurrent re-deliveries are safe.
    sqlx::query(
        "INSERT INTO stripe_events (stripe_event_id, event_type, payload)
         VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(event_id)
    .bind(event_type)
    .bind(data)
    .execute(db)
    .await?;

    dispatch_event(db, event_type, data, price_pro, price_business).await?;
    Ok(true)
}

async fn dispatch_event(
    db: &PgPool,
    event_type: &str,
    data: &Value,
    price_pro: &str,
    price_business: &str,
) -> Result<()> {
    match event_type {
        "customer.subscription.created" | "customer.subscription.updated" => {
            let sub = &data["object"];
            let customer_id = sub["customer"].as_str().unwrap_or("");
            let sub_id = sub["id"].as_str().unwrap_or("");
            let status = sub["status"].as_str().unwrap_or("unknown");
            let price_id = sub["items"]["data"][0]["price"]["id"]
                .as_str()
                .unwrap_or("");

            let tier = tier_for_price(price_id, price_pro, price_business);

            sqlx::query(
                r#"
                UPDATE users
                SET stripe_subscription_id = $1,
                    subscription_status    = $2,
                    tier                   = $3,
                    updated_at             = now()
                WHERE stripe_customer_id = $4
                "#,
            )
            .bind(sub_id)
            .bind(status)
            .bind(&tier)
            .bind(customer_id)
            .execute(db)
            .await?;

            info!(customer_id, sub_id, status, tier = tier.as_str(), "subscription updated");
        }

        "customer.subscription.deleted" => {
            let sub = &data["object"];
            let customer_id = sub["customer"].as_str().unwrap_or("");

            sqlx::query(
                r#"
                UPDATE users
                SET stripe_subscription_id = NULL,
                    subscription_status    = 'cancelled',
                    tier                   = 'free',
                    updated_at             = now()
                WHERE stripe_customer_id = $1
                "#,
            )
            .bind(customer_id)
            .execute(db)
            .await?;

            info!(customer_id, "subscription cancelled, tier reset to free");
        }

        "invoice.payment_failed" => {
            let customer_id = data["object"]["customer"].as_str().unwrap_or("");
            sqlx::query(
                "UPDATE users SET subscription_status = 'past_due', updated_at = now()
                 WHERE stripe_customer_id = $1",
            )
            .bind(customer_id)
            .execute(db)
            .await?;
            warn!(customer_id, "invoice payment failed, marked past_due");
        }

        "invoice.payment_succeeded" => {
            let customer_id = data["object"]["customer"].as_str().unwrap_or("");
            sqlx::query(
                "UPDATE users SET subscription_status = 'active', updated_at = now()
                 WHERE stripe_customer_id = $1",
            )
            .bind(customer_id)
            .execute(db)
            .await?;
            info!(customer_id, "invoice payment succeeded");
        }

        other => {
            info!(event_type = other, "unhandled stripe event type");
        }
    }

    Ok(())
}

fn tier_for_price(price_id: &str, price_pro: &str, price_business: &str) -> SubscriptionTier {
    if !price_business.is_empty() && price_id == price_business {
        SubscriptionTier::Business
    } else if !price_pro.is_empty() && price_id == price_pro {
        SubscriptionTier::Pro
    } else {
        SubscriptionTier::Pro // default paid tier if price not recognised
    }
}
