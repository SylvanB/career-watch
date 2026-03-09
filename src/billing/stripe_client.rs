//! Minimal Stripe API client using `reqwest`.
//!
//! Covers the three operations needed for SaaS billing:
//! - Creating a customer record
//! - Starting a Checkout session for new subscriptions
//! - Opening a Billing Portal session for self-serve management
//!
//! Webhook signature verification is also provided here.

use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::Sha256;
use std::time::Duration;

type HmacSha256 = Hmac<Sha256>;

const STRIPE_API: &str = "https://api.stripe.com";

/// Minimal Stripe REST client.
#[derive(Clone)]
pub struct StripeClient {
    client: Client,
    secret_key: String,
}

impl StripeClient {
    pub fn new(secret_key: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build Stripe HTTP client")?;
        Ok(Self { client, secret_key })
    }

    /// POST a URL-encoded form to a Stripe endpoint and return the JSON body.
    async fn post(&self, path: &str, params: &[(&str, &str)]) -> Result<serde_json::Value> {
        let url = format!("{}{}", STRIPE_API, path);
        let resp = self
            .client
            .post(&url)
            .basic_auth(&self.secret_key, Some(""))
            .form(params)
            .send()
            .await
            .context("Stripe request failed")?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await.context("Stripe response decode failed")?;

        if !status.is_success() {
            let msg = body["error"]["message"]
                .as_str()
                .unwrap_or("unknown Stripe error")
                .to_string();
            return Err(anyhow::anyhow!("Stripe {} error: {}", status, msg));
        }

        Ok(body)
    }

    /// Create a Stripe customer for a new user. Returns the `cus_*` customer ID.
    pub async fn create_customer(&self, email: &str, user_id: &str) -> Result<String> {
        let resp = self
            .post(
                "/v1/customers",
                &[("email", email), ("metadata[user_id]", user_id)],
            )
            .await?;
        Ok(resp["id"]
            .as_str()
            .context("missing customer id in Stripe response")?
            .to_string())
    }

    /// Create a Checkout session for a subscription upgrade.
    ///
    /// Returns the hosted Checkout URL to redirect the user to.
    pub async fn create_checkout_session(
        &self,
        customer_id: &str,
        price_id: &str,
        success_url: &str,
        cancel_url: &str,
    ) -> Result<String> {
        let resp = self
            .post(
                "/v1/checkout/sessions",
                &[
                    ("mode", "subscription"),
                    ("customer", customer_id),
                    ("line_items[0][price]", price_id),
                    ("line_items[0][quantity]", "1"),
                    ("success_url", success_url),
                    ("cancel_url", cancel_url),
                    ("allow_promotion_codes", "true"),
                ],
            )
            .await?;
        Ok(resp["url"]
            .as_str()
            .context("missing checkout url in Stripe response")?
            .to_string())
    }

    /// Create a Billing Portal session for subscription management.
    ///
    /// Returns the portal URL to redirect the user to.
    pub async fn create_portal_session(
        &self,
        customer_id: &str,
        return_url: &str,
    ) -> Result<String> {
        let resp = self
            .post(
                "/v1/billing_portal/sessions",
                &[("customer", customer_id), ("return_url", return_url)],
            )
            .await?;
        Ok(resp["url"]
            .as_str()
            .context("missing portal url in Stripe response")?
            .to_string())
    }
}

/// Verify a Stripe webhook `Stripe-Signature` header against the raw request body.
///
/// **Important:** `payload` must be the raw bytes of the request body *before*
/// any JSON parsing. Parsing first changes the byte representation and breaks
/// the HMAC check.
///
/// Returns `true` when the signature is valid.
pub fn verify_webhook_signature(payload: &[u8], sig_header: &str, secret: &str) -> bool {
    let mut timestamp = "";
    let mut expected: Vec<&str> = Vec::new();

    for part in sig_header.split(',') {
        if let Some(t) = part.strip_prefix("t=") {
            timestamp = t;
        } else if let Some(s) = part.strip_prefix("v1=") {
            expected.push(s);
        }
    }

    if timestamp.is_empty() || expected.is_empty() {
        return false;
    }

    let signed = format!("{}.{}", timestamp, String::from_utf8_lossy(payload));

    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(signed.as_bytes());
    let computed: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    expected.iter().any(|sig| *sig == computed)
}

#[cfg(test)]
mod tests {
    use super::verify_webhook_signature;

    /// Example from https://stripe.com/docs/webhooks/signatures
    #[test]
    fn rejects_bad_signature() {
        let payload = b"{}";
        let sig = "t=1492774577,v1=bad_sig";
        assert!(!verify_webhook_signature(payload, sig, "secret"));
    }

    #[test]
    fn rejects_empty_header() {
        assert!(!verify_webhook_signature(b"{}", "", "secret"));
    }
}
