//! Hiring analytics endpoints.
//!
//! GET /api/companies/:id/metrics  — all derived signals for one company
//! GET /api/companies/:id/jobs     — paginated raw job list

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::analytics::metrics::compute_metrics;
use crate::models::AuthUser;
use crate::AppState;

// ── Company metrics ───────────────────────────────────────────────────────────

pub async fn company_metrics(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthUser>,
    Path(company_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Confirm the company exists and is accessible
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM companies WHERE id = $1 AND status = 'active')",
    )
    .bind(company_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if !exists {
        return Err(StatusCode::NOT_FOUND);
    }

    let metrics = compute_metrics(&state.db, company_id)
        .await
        .map_err(|e| {
            tracing::error!("metrics computation failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(serde_json::to_value(metrics).unwrap_or_default()))
}

// ── Job list ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct JobListParams {
    #[serde(default)]
    pub status: Option<String>,       // active | filled | all (default: active)
    #[serde(default)]
    pub department: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 { 50 }

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct JobRow {
    pub id: Uuid,
    pub title: String,
    pub department_category: String,
    pub raw_location: Option<String>,
    pub location_region: String,
    pub is_remote: bool,
    pub url: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub status: String,
    pub filled_at: Option<DateTime<Utc>>,
    pub time_to_fill_days: Option<i32>,
}

pub async fn company_jobs(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthUser>,
    Path(company_id): Path<Uuid>,
    Query(params): Query<JobListParams>,
) -> Result<Json<Vec<JobRow>>, StatusCode> {
    let status_filter = params.status.as_deref().unwrap_or("active");

    let rows = sqlx::query_as::<_, JobRow>(
        r#"
        SELECT id, title, department_category, raw_location, location_region,
               is_remote, url, first_seen_at, last_seen_at, status,
               filled_at, time_to_fill_days
        FROM jobs
        WHERE company_id = $1
          AND ($2::text = 'all' OR status = $2)
          AND ($3::text IS NULL OR department_category = $3)
          AND ($4::text IS NULL OR location_region     = $4)
        ORDER BY first_seen_at DESC
        LIMIT $5 OFFSET $6
        "#,
    )
    .bind(company_id)
    .bind(status_filter)
    .bind(params.department.as_deref())
    .bind(params.region.as_deref())
    .bind(params.limit.clamp(1, 200))
    .bind(params.offset)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(rows))
}

// ── Cross-company signals (for watched companies) ─────────────────────────────

/// Hiring signals across all companies the current user is watching.
/// Powers the signals dashboard view.
#[derive(Serialize)]
pub struct HiringSignals {
    pub spikes:           Vec<SpikeSignal>,
    pub new_regions:      Vec<RegionSignal>,
    pub dept_expansions:  Vec<ExpansionSignal>,
}

#[derive(Serialize)]
pub struct SpikeSignal {
    pub company_id:   Uuid,
    pub company_name: String,
    pub multiplier:   f64,
    pub new_jobs_7d:  i64,
}

#[derive(Serialize)]
pub struct RegionSignal {
    pub company_id:   Uuid,
    pub company_name: String,
    pub region:       String,
}

#[derive(Serialize)]
pub struct ExpansionSignal {
    pub company_id:   Uuid,
    pub company_name: String,
    pub department:   String,
    pub growth_pct:   i64,
    pub count_now:    i64,
}

pub async fn hiring_signals(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<HiringSignals>, StatusCode> {
    // Fetch the companies the user is watching (via the catalog)
    let watched: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT c.id, c.name
        FROM companies c
        JOIN watchers w ON w.company_id = c.id
        WHERE w.user_id = $1 AND w.active = true AND c.status = 'active'
        "#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut spikes:       Vec<SpikeSignal>     = Vec::new();
    let mut new_regions:  Vec<RegionSignal>    = Vec::new();
    let mut expansions:   Vec<ExpansionSignal> = Vec::new();

    for (company_id, company_name) in &watched {
        // Only compute for companies with tracked jobs
        let job_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE company_id = $1",
        )
        .bind(company_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(Some(0))
        .unwrap_or(0);

        if job_count < 5 {
            continue; // not enough data yet
        }

        if let Ok(m) = compute_metrics(&state.db, *company_id).await {
            if m.spike.is_spike {
                spikes.push(SpikeSignal {
                    company_id:   *company_id,
                    company_name: company_name.clone(),
                    multiplier:   m.spike.multiplier.unwrap_or(0.0),
                    new_jobs_7d:  m.spike.new_jobs_7d,
                });
            }
            for region in m.new_regions_30d {
                new_regions.push(RegionSignal {
                    company_id:   *company_id,
                    company_name: company_name.clone(),
                    region,
                });
            }
            for dept in m.expanding_departments {
                expansions.push(ExpansionSignal {
                    company_id:   *company_id,
                    company_name: company_name.clone(),
                    department:   dept.department,
                    growth_pct:   dept.growth_pct,
                    count_now:    dept.count_now,
                });
            }
        }
    }

    // Sort by signal strength
    spikes.sort_by(|a, b| b.multiplier.partial_cmp(&a.multiplier).unwrap_or(std::cmp::Ordering::Equal));
    expansions.sort_by(|a, b| b.growth_pct.cmp(&a.growth_pct));

    Ok(Json(HiringSignals { spikes, new_regions, dept_expansions: expansions }))
}
