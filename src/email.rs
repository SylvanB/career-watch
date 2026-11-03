//! Email sending utilities for Career Watch.
//!
//! Wraps `lettre` to send all transactional emails: change notifications,
//! welcome, verification, password reset, billing alerts, and upgrade prompts.

use crate::config::AppConfig;
use crate::models::JobPosting;
use anyhow::Context;
use lettre::message::{header, Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

/// Asynchronous SMTP email client configured from application settings.
#[derive(Clone)]
pub struct EmailClient {
    mailer: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl EmailClient {
    pub fn from_config(config: &AppConfig) -> anyhow::Result<Self> {
        let has_creds = !config.smtp_user.is_empty();
        let creds = Credentials::new(config.smtp_user.clone(), config.smtp_pass.clone());

        let mailer = if config.smtp_tls {
            let mut b = AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp_host)
                .context("failed to configure SMTP relay")?
                .port(config.smtp_port);
            if has_creds {
                b = b.credentials(creds);
            }
            b.build()
        } else {
            let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host)
                .port(config.smtp_port);
            if has_creds {
                b = b.credentials(creds);
            }
            b.build()
        };

        let from: Mailbox = config
            .smtp_from
            .parse()
            .context("invalid SMTP_FROM email address")?;

        Ok(Self { mailer, from })
    }

    async fn send(&self, to: &str, subject: &str, body: String) -> anyhow::Result<()> {
        let to: Mailbox = to.parse().context("invalid recipient email")?;
        let message = Message::builder()
            .from(self.from.clone())
            .to(to)
            .header(header::ContentType::TEXT_PLAIN)
            .subject(subject)
            .body(body)
            .context("failed to build email message")?;
        self.mailer
            .send(message)
            .await
            .context("failed to send email")?;
        Ok(())
    }

    // ── Career page change notifications ──────────────────────────────────────

    /// Notify a user that a watched career page has changed.
    pub async fn send_change_email(
        &self,
        to: &str,
        company_name: &str,
        url: &str,
        added: &[JobPosting],
        removed: &[JobPosting],
    ) -> anyhow::Result<()> {
        let subject = format!("Career page changed — {}", company_name);
        let mut body = String::new();

        if added.is_empty() && removed.is_empty() {
            body.push_str(&format!(
                "The watched career page for {company_name} has changed.\n\n"
            ));
        } else {
            body.push_str(&format!("New changes detected for {company_name}:\n\n"));

            if !added.is_empty() {
                body.push_str("New jobs:\n");
                for job in added {
                    body.push_str(&format!("  + {}", job.title));
                    if let Some(ref p) = job.posted_text {
                        body.push_str(&format!(" ({})", p));
                    }
                    body.push('\n');
                }
                body.push('\n');
            }

            if !removed.is_empty() {
                body.push_str("Removed jobs:\n");
                for job in removed {
                    body.push_str(&format!("  - {}\n", job.title));
                }
                body.push('\n');
            }
        }

        body.push_str(&format!("View: {url}\n"));

        self.send(to, &subject, body).await
    }

    // ── Auth emails ───────────────────────────────────────────────────────────

    /// Welcome email sent after successful registration.
    pub async fn send_welcome_email(&self, to: &str) -> anyhow::Result<()> {
        let body = "Welcome to Career Watch!\n\n\
             You're now set up to monitor company career pages and receive \
             instant alerts when job listings change.\n\n\
             Get started by adding your first watcher in your dashboard.\n\n\
             — The Career Watch team\n"
            .to_string();
        self.send(to, "Welcome to Career Watch", body).await
    }

    /// Email verification link.
    pub async fn send_verification_email(&self, to: &str, verify_url: &str) -> anyhow::Result<()> {
        let body = format!(
            "Please verify your Career Watch email address.\n\n\
             Click the link below to confirm your email. \
             This link expires in 24 hours.\n\n\
             {verify_url}\n\n\
             If you didn't create an account, you can ignore this email.\n"
        );
        self.send(to, "Verify your Career Watch email", body).await
    }

    /// Password reset link. Expires in 1 hour.
    pub async fn send_password_reset_email(&self, to: &str, reset_url: &str) -> anyhow::Result<()> {
        let body = format!(
            "You requested a password reset for your Career Watch account.\n\n\
             Click the link below to set a new password. \
             This link expires in 1 hour.\n\n\
             {reset_url}\n\n\
             If you didn't request this, you can safely ignore this email.\n"
        );
        self.send(to, "Reset your Career Watch password", body)
            .await
    }

    // ── Billing emails ────────────────────────────────────────────────────────

    /// Payment failure alert. Sent when Stripe marks an invoice as past_due.
    pub async fn send_payment_failed_email(
        &self,
        to: &str,
        portal_url: &str,
    ) -> anyhow::Result<()> {
        let body = format!(
            "We were unable to process your Career Watch subscription payment.\n\n\
             Please update your payment method to keep your subscription active:\n\n\
             {portal_url}\n\n\
             If we can't collect payment your account will revert to the Free plan.\n"
        );
        self.send(to, "Action required: Career Watch payment failed", body)
            .await
    }

    /// Upgrade prompt sent when a user approaches their watcher limit.
    pub async fn send_upgrade_prompt_email(
        &self,
        to: &str,
        current_count: i64,
        max_count: i64,
        upgrade_url: &str,
    ) -> anyhow::Result<()> {
        let body = format!(
            "You're using {current_count} of your {max_count} Career Watch watchers.\n\n\
             Upgrade your plan to monitor more companies:\n\n\
             {upgrade_url}\n"
        );
        self.send(
            to,
            "You're approaching your Career Watch watcher limit",
            body,
        )
        .await
    }
}
