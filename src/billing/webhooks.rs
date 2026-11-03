//! Stripe webhook event processing.
//!
//! Events are processed idempotently: each `stripe_event_id` is recorded in
//! the `stripe_events` table so re-delivered events are no-ops.

use anyhow::Result;
use serde_json::Value;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::email::EmailClient;
use crate::models::SubscriptionTier;

pub struct WebhookContext<'a> {
    pub db: &'a PgPool,
    pub email: &'a EmailClient,
    pub price_business: &'a str,
    pub app_base_url: &'a str,
}

/// Record and process a verified Stripe webhook event.
///
/// Returns `Ok(false)` if the event has already been processed (idempotent
/// skip), `Ok(true)` on successful first processing.
pub async fn handle_event(
    context: WebhookContext<'_>,
    event_id: &str,
    event_type: &str,
    data: &Value,
) -> Result<bool> {
    // Idempotency: skip already-processed events.
    let already_seen = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM stripe_events WHERE stripe_event_id = $1)",
    )
    .bind(event_id)
    .fetch_one(context.db)
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
    .execute(context.db)
    .await?;

    dispatch_event(
        context.db,
        context.email,
        event_type,
        data,
        context.price_business,
        context.app_base_url,
    )
    .await?;
    Ok(true)
}

async fn dispatch_event(
    db: &PgPool,
    email: &EmailClient,
    event_type: &str,
    data: &Value,
    price_business: &str,
    app_base_url: &str,
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

            let tier = tier_for_price(price_id, price_business);

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

            info!(
                customer_id,
                sub_id,
                status,
                tier = tier.as_str(),
                "subscription updated"
            );
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

            // Notify the user so they can update their payment method.
            let user_email = sqlx::query_scalar::<_, String>(
                "SELECT email FROM users WHERE stripe_customer_id = $1",
            )
            .bind(customer_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();

            if let Some(addr) = user_email {
                let portal_url = format!("{}/account", app_base_url);
                if let Err(e) = email.send_payment_failed_email(&addr, &portal_url).await {
                    warn!(customer_id, "failed to send payment failed email: {e}");
                }
            }
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

fn tier_for_price(price_id: &str, price_business: &str) -> SubscriptionTier {
    if !price_business.is_empty() && price_id == price_business {
        SubscriptionTier::Business
    } else {
        SubscriptionTier::Pro
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_pro_price_id() {
        assert_eq!(
            tier_for_price("price_pro_123", "price_biz_456"),
            SubscriptionTier::Pro
        );
    }

    #[test]
    fn maps_business_price_id() {
        assert_eq!(
            tier_for_price("price_biz_456", "price_biz_456"),
            SubscriptionTier::Business
        );
    }

    #[test]
    fn unknown_price_defaults_to_pro() {
        assert_eq!(
            tier_for_price("price_unknown", "price_biz_456"),
            SubscriptionTier::Pro
        );
    }

    #[test]
    fn empty_configured_prices_default_to_pro() {
        // Stripe price IDs not configured yet — default to Pro
        assert_eq!(tier_for_price("price_anything", ""), SubscriptionTier::Pro);
    }

    #[test]
    fn business_takes_priority_over_pro_match() {
        // Edge case: same price ID configured for both (shouldn't happen, but safe)
        let same = "price_same";
        assert_eq!(tier_for_price(same, same), SubscriptionTier::Business);
    }
}
