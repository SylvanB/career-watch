//! Company catalog endpoints.
//!
//! The catalog surfaces auto-discovered companies from the discovery pipeline.
//! Authenticated users can browse regardless of email verification status.
//! Watching a company requires a verified email address.
//!
//! GET  /api/companies            — paginated, searchable catalog
//! POST /api/companies/:id/watch  — subscribe (idempotent, requires verified email)
//! DELETE /api/companies/:id/watch — unsubscribe

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::AuthUser;
use crate::AppState;

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CatalogParams {
    /// Full-text search on company name (case-insensitive).
    pub q: Option<String>,
    /// Filter by ATS platform slug: greenhouse | lever | workday | ashby | etc.
    pub platform: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 { 48 }

/// A company from the discovery catalog, enriched with the current user's
/// watch status so the frontend can render the toggle without a second call.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CatalogItem {
    pub id: Uuid,
    pub name: String,
    pub domain: String,
    pub career_url: String,
    pub platform: String,
    pub platform_id: Option<String>,
    /// Set when the current user is watching this company.
    pub watcher_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct CatalogResponse {
    pub items: Vec<CatalogItem>,
    pub total: i64,
    pub has_more: bool,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// List the company catalog.
///
/// Available to any authenticated user, including those who haven't verified
/// their email yet — they can browse but not watch.
pub async fn list_companies(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(params): Query<CatalogParams>,
) -> Result<Json<CatalogResponse>, StatusCode> {
    let limit = params.limit.clamp(1, 100);
    let search = params.q.as_deref().filter(|s| !s.is_empty());
    let platform = params.platform.as_deref().filter(|s| !s.is_empty());

    // Count total matching rows for pagination metadata.
    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM companies
        WHERE status = 'active'
          AND career_url IS NOT NULL
          AND ($1::text IS NULL OR name ILIKE '%' || $1 || '%')
          AND ($2::text IS NULL OR platform = $2)
        "#,
    )
    .bind(search)
    .bind(platform)
    .fetch_one(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Fetch page, left-joining on the current user's active watchers so we
    // know which companies they're already watching.
    let items = sqlx::query_as::<_, CatalogItem>(
        r#"
        SELECT
            c.id,
            c.name,
            c.domain,
            c.career_url,
            c.platform,
            c.platform_id,
            w.id AS watcher_id
        FROM companies c
        LEFT JOIN watchers w
            ON  w.company_id = c.id
            AND w.user_id    = $1
            AND w.active     = true
        WHERE c.status     = 'active'
          AND c.career_url IS NOT NULL
          AND ($2::text IS NULL OR c.name ILIKE '%' || $2 || '%')
          AND ($3::text IS NULL OR c.platform = $3)
        ORDER BY c.name ASC
        LIMIT $4 OFFSET $5
        "#,
    )
    .bind(user.id)
    .bind(search)
    .bind(platform)
    .bind(limit)
    .bind(params.offset)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let has_more = params.offset + limit < total;

    Ok(Json(CatalogResponse { items, total, has_more }))
}

/// Watch a company from the catalog (idempotent).
///
/// If the user already has an active watcher for this company, returns 200
/// with the existing watcher ID rather than creating a duplicate.
/// Requires a verified email address.
pub async fn watch_company(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(company_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !user.email_verified {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "verify your email address before adding watchers"
            })),
        ));
    }

    // Look up the company.
    let company = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT name, career_url, platform, platform_id FROM companies WHERE id = $1 AND status = 'active'",
    )
    .bind(company_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"internal error"}))))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":"company not found"}))))?;

    let (name, career_url, _platform, _platform_id) = company;

    // Idempotency: return the existing watcher if already watching.
    if let Some(watcher_id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM watchers WHERE company_id = $1 AND user_id = $2 AND active = true",
    )
    .bind(company_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"internal error"}))))?
    {
        return Ok(Json(serde_json::json!({ "watcher_id": watcher_id })));
    }

    // Enforce tier watcher limit.
    let max = user.tier.max_watchers();
    let current: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM watchers WHERE user_id = $1 AND active = true",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"internal error"}))))?;

    if current >= max {
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            Json(serde_json::json!({
                "error": "watcher limit reached for your plan",
                "limit": max,
                "tier": user.tier.as_str(),
            })),
        ));
    }

    // Create the watcher linked to the catalog company.
    let watcher_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO watchers (id, user_id, company_id, company_name, url, email)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(watcher_id)
    .bind(user.id)
    .bind(company_id)
    .bind(&name)
    .bind(&career_url)
    .bind(&user.email)
    .execute(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"internal error"}))))?;

    // Nudge if approaching the limit (≥ 80%).
    let after = current + 1;
    if after >= (max * 4 / 5) && after < max && user.tier.monthly_price_usd().is_none() {
        let upgrade_url = format!("{}/pricing", state.config.app_base_url);
        let _ = state
            .email
            .send_upgrade_prompt_email(&user.email, after, max, &upgrade_url)
            .await;
    }

    Ok(Json(serde_json::json!({ "watcher_id": watcher_id })))
}

/// Unwatch a company (soft-deletes the watcher).
pub async fn unwatch_company(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(company_id): Path<Uuid>,
) -> StatusCode {
    let _ = sqlx::query(
        "UPDATE watchers SET active = false WHERE company_id = $1 AND user_id = $2",
    )
    .bind(company_id)
    .bind(user.id)
    .execute(&state.db)
    .await;

    StatusCode::NO_CONTENT
}
