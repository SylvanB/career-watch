//! Metric computation queries over the `jobs` and `company_hiring_snapshots`
//! tables. Uses runtime-checked queries (not macros) so the code compiles
//! before the migration has been applied.

use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct HiringVelocity {
    pub jobs_per_week_7d:  f64,
    pub jobs_per_week_28d: f64,
    /// "up" | "down" | "stable"
    pub trend: &'static str,
}

#[derive(Debug, Serialize)]
pub struct HiringSpike {
    pub is_spike:               bool,
    pub multiplier:             Option<f64>,
    pub new_jobs_7d:            i64,
    pub baseline_jobs_per_week: f64,
}

#[derive(Debug, Serialize)]
pub struct DepartmentBreakdown {
    pub engineering:      i64,
    pub product:          i64,
    pub sales:            i64,
    pub marketing:        i64,
    pub design:           i64,
    pub data:             i64,
    pub operations:       i64,
    pub people:           i64,
    pub finance:          i64,
    pub legal:            i64,
    pub customer_success: i64,
    pub other:            i64,
    pub total:            i64,
}

#[derive(Debug, Serialize)]
pub struct GeoBreakdown {
    pub remote:  i64,
    pub us:      i64,
    pub europe:  i64,
    pub apac:    i64,
    pub latam:   i64,
    pub unknown: i64,
}

#[derive(Debug, Serialize)]
pub struct WeeklyCount {
    pub week_start: String,
    pub new_jobs:   i64,
}

#[derive(Debug, Serialize)]
pub struct DeptGrowth {
    pub department: String,
    pub count_now:  i64,
    pub count_60d:  i64,
    pub growth_pct: i64,
}

#[derive(Debug, Serialize)]
pub struct CompanyMetrics {
    pub company_id:            Uuid,
    pub active_jobs:           i64,
    pub total_jobs_tracked:    i64,
    pub jobs_filled_30d:       i64,
    pub avg_time_to_fill_days: Option<f64>,
    pub velocity:              HiringVelocity,
    pub spike:                 HiringSpike,
    pub departments:           DepartmentBreakdown,
    pub geo:                   GeoBreakdown,
    pub weekly_new_jobs:       Vec<WeeklyCount>,
    pub new_regions_30d:       Vec<String>,
    pub expanding_departments: Vec<DeptGrowth>,
}

// ── Top-level entry point ─────────────────────────────────────────────────────

pub async fn compute_metrics(db: &PgPool, company_id: Uuid) -> Result<CompanyMetrics> {
    let (active_jobs, total_tracked, filled_30d, avg_ttf) = base_counts(db, company_id).await?;
    let velocity  = compute_velocity(db, company_id).await?;
    let spike     = compute_spike(db, company_id, &velocity).await?;
    let depts     = dept_breakdown(db, company_id).await?;
    let geo       = geo_breakdown(db, company_id).await?;
    let weekly    = weekly_new_jobs(db, company_id, 12).await?;
    let new_reg   = new_regions_30d(db, company_id).await?;
    let expanding = expanding_departments(db, company_id).await?;

    Ok(CompanyMetrics {
        company_id,
        active_jobs,
        total_jobs_tracked: total_tracked,
        jobs_filled_30d: filled_30d,
        avg_time_to_fill_days: avg_ttf,
        velocity,
        spike,
        departments: depts,
        geo,
        weekly_new_jobs: weekly,
        new_regions_30d: new_reg,
        expanding_departments: expanding,
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn base_counts(db: &PgPool, cid: Uuid) -> Result<(i64, i64, i64, Option<f64>)> {
    let row: (i64, i64, i64, Option<f64>) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*)                       FILTER (WHERE status = 'active'),
            COUNT(*),
            COUNT(*)                       FILTER (WHERE filled_at >= now() - interval '30 days'),
            AVG(time_to_fill_days::float8) FILTER (WHERE time_to_fill_days IS NOT NULL)
        FROM jobs WHERE company_id = $1
        "#,
    )
    .bind(cid)
    .fetch_one(db)
    .await?;
    Ok(row)
}

async fn compute_velocity(db: &PgPool, cid: Uuid) -> Result<HiringVelocity> {
    let row: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE first_seen_at >= now() - interval  '7 days'),
            COUNT(*) FILTER (WHERE first_seen_at >= now() - interval '28 days')
        FROM jobs WHERE company_id = $1
        "#,
    )
    .bind(cid)
    .fetch_one(db)
    .await?;

    let v7  = row.0 as f64;
    let v28 = row.1 as f64 / 4.0;

    let trend = if v28 < 0.1 {
        "stable"
    } else if v7 > v28 * 1.25 {
        "up"
    } else if v7 < v28 * 0.75 {
        "down"
    } else {
        "stable"
    };

    Ok(HiringVelocity { jobs_per_week_7d: v7, jobs_per_week_28d: v28, trend })
}

async fn compute_spike(db: &PgPool, cid: Uuid, velocity: &HiringVelocity) -> Result<HiringSpike> {
    let historical: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM jobs
        WHERE company_id    = $1
          AND first_seen_at >= now() - interval '90 days'
          AND first_seen_at <  now() - interval  '7 days'
        "#,
    )
    .bind(cid)
    .fetch_one(db)
    .await?;

    let baseline = historical as f64 / (83.0 / 7.0);
    let v7 = velocity.jobs_per_week_7d;

    let (is_spike, multiplier) = if baseline < 0.5 {
        (false, None)
    } else {
        let m = (v7 / baseline * 10.0).round() / 10.0;
        (m >= 1.5, Some(m))
    };

    Ok(HiringSpike {
        is_spike,
        multiplier,
        new_jobs_7d: v7 as i64,
        baseline_jobs_per_week: (baseline * 10.0).round() / 10.0,
    })
}

async fn dept_breakdown(db: &PgPool, cid: Uuid) -> Result<DepartmentBreakdown> {
    let rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT department_category, COUNT(*) FROM jobs WHERE company_id = $1 AND status = 'active' GROUP BY 1",
    )
    .bind(cid)
    .fetch_all(db)
    .await?;

    let mut d = DepartmentBreakdown {
        engineering: 0, product: 0, sales: 0, marketing: 0,
        design: 0, data: 0, operations: 0, people: 0,
        finance: 0, legal: 0, customer_success: 0, other: 0, total: 0,
    };

    for (cat, c) in rows {
        match cat.as_deref() {
            Some("engineering")      => d.engineering      += c,
            Some("product")          => d.product          += c,
            Some("sales")            => d.sales            += c,
            Some("marketing")        => d.marketing        += c,
            Some("design")           => d.design           += c,
            Some("data")             => d.data             += c,
            Some("operations")       => d.operations       += c,
            Some("people")           => d.people           += c,
            Some("finance")          => d.finance          += c,
            Some("legal")            => d.legal            += c,
            Some("customer_success") => d.customer_success += c,
            _                        => d.other            += c,
        }
    }

    d.total = d.engineering + d.product + d.sales + d.marketing + d.design
            + d.data + d.operations + d.people + d.finance + d.legal
            + d.customer_success + d.other;
    Ok(d)
}

async fn geo_breakdown(db: &PgPool, cid: Uuid) -> Result<GeoBreakdown> {
    let rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT location_region, COUNT(*) FROM jobs WHERE company_id = $1 AND status = 'active' GROUP BY 1",
    )
    .bind(cid)
    .fetch_all(db)
    .await?;

    let mut g = GeoBreakdown { remote: 0, us: 0, europe: 0, apac: 0, latam: 0, unknown: 0 };

    for (reg, c) in rows {
        match reg.as_deref() {
            Some("remote")  => g.remote  += c,
            Some("us")      => g.us      += c,
            Some("europe")  => g.europe  += c,
            Some("apac")    => g.apac    += c,
            Some("latam")   => g.latam   += c,
            _               => g.unknown += c,
        }
    }
    Ok(g)
}

async fn weekly_new_jobs(db: &PgPool, cid: Uuid, weeks: i32) -> Result<Vec<WeeklyCount>> {
    #[derive(sqlx::FromRow)]
    struct Row { week_start: NaiveDate, count: i64 }

    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT date_trunc('week', first_seen_at)::date AS week_start, COUNT(*) AS count
        FROM jobs
        WHERE company_id    = $1
          AND first_seen_at >= now() - ($2::int * interval '1 week')
        GROUP BY 1
        ORDER BY 1
        "#,
    )
    .bind(cid)
    .bind(weeks)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().map(|r| WeeklyCount {
        week_start: r.week_start.format("%Y-%m-%d").to_string(),
        new_jobs:   r.count,
    }).collect())
}

async fn new_regions_30d(db: &PgPool, cid: Uuid) -> Result<Vec<String>> {
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT location_region
        FROM jobs
        WHERE company_id = $1
          AND location_region != 'unknown'
          AND first_seen_at >= now() - interval '30 days'
          AND location_region NOT IN (
              SELECT DISTINCT location_region FROM jobs
              WHERE company_id = $1 AND first_seen_at < now() - interval '30 days'
          )
        "#,
    )
    .bind(cid)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().filter_map(|(r,)| r).collect())
}

async fn expanding_departments(db: &PgPool, cid: Uuid) -> Result<Vec<DeptGrowth>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        dept:       Option<String>,
        count_now:  i64,
        count_then: i64,
        growth_pct: i64,
    }

    let rows: Vec<Row> = sqlx::query_as(
        r#"
        WITH now_counts AS (
            SELECT department_category, COUNT(*) AS cnt
            FROM jobs WHERE company_id = $1 AND status = 'active'
            GROUP BY department_category
        ),
        then_counts AS (
            SELECT department_category, COUNT(*) AS cnt
            FROM jobs
            WHERE company_id = $1 AND status = 'active'
              AND first_seen_at <= now() - interval '60 days'
            GROUP BY department_category
        )
        SELECT
            n.department_category                                                       AS dept,
            n.cnt                                                                       AS count_now,
            COALESCE(t.cnt, 0)                                                          AS count_then,
            CASE WHEN COALESCE(t.cnt,0)=0 THEN 100
                 ELSE ((n.cnt - COALESCE(t.cnt,0))::float8 / COALESCE(t.cnt,0) * 100)::bigint
            END                                                                         AS growth_pct
        FROM now_counts n
        LEFT JOIN then_counts t USING (department_category)
        WHERE n.cnt >= 3
          AND (COALESCE(t.cnt,0) = 0 OR (n.cnt - COALESCE(t.cnt,0))::float8 / COALESCE(t.cnt,0) > 0.2)
        ORDER BY growth_pct DESC
        "#,
    )
    .bind(cid)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().filter_map(|r| {
        Some(DeptGrowth {
            department: r.dept?,
            count_now:  r.count_now,
            count_60d:  r.count_then,
            growth_pct: r.growth_pct,
        })
    }).collect())
}

// ── Daily snapshot ────────────────────────────────────────────────────────────

pub async fn upsert_daily_snapshot(db: &PgPool, company_id: Uuid, date: NaiveDate) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO company_hiring_snapshots (
            id, company_id, snapshot_date,
            active_count, new_count, filled_count,
            eng_count, product_count, sales_count, marketing_count,
            design_count, data_count, ops_count, people_count, other_count,
            remote_count, us_count, europe_count, apac_count, other_geo_count
        )
        SELECT
            gen_random_uuid(), $1, $2,
            COUNT(*)  FILTER (WHERE status = 'active'),
            COUNT(*)  FILTER (WHERE first_seen_at::date = $2),
            COUNT(*)  FILTER (WHERE filled_at::date     = $2),
            COUNT(*)  FILTER (WHERE status='active' AND department_category='engineering'),
            COUNT(*)  FILTER (WHERE status='active' AND department_category='product'),
            COUNT(*)  FILTER (WHERE status='active' AND department_category='sales'),
            COUNT(*)  FILTER (WHERE status='active' AND department_category='marketing'),
            COUNT(*)  FILTER (WHERE status='active' AND department_category='design'),
            COUNT(*)  FILTER (WHERE status='active' AND department_category='data'),
            COUNT(*)  FILTER (WHERE status='active' AND department_category='operations'),
            COUNT(*)  FILTER (WHERE status='active' AND department_category='people'),
            COUNT(*)  FILTER (WHERE status='active' AND department_category NOT IN
                              ('engineering','product','sales','marketing','design','data','operations','people')),
            COUNT(*)  FILTER (WHERE status='active' AND is_remote=true),
            COUNT(*)  FILTER (WHERE status='active' AND location_region='us'),
            COUNT(*)  FILTER (WHERE status='active' AND location_region='europe'),
            COUNT(*)  FILTER (WHERE status='active' AND location_region='apac'),
            COUNT(*)  FILTER (WHERE status='active' AND location_region NOT IN ('remote','us','europe','apac'))
        FROM jobs WHERE company_id = $1
        ON CONFLICT (company_id, snapshot_date) DO UPDATE SET
            active_count    = EXCLUDED.active_count,
            new_count       = EXCLUDED.new_count,
            filled_count    = EXCLUDED.filled_count,
            eng_count       = EXCLUDED.eng_count,
            product_count   = EXCLUDED.product_count,
            sales_count     = EXCLUDED.sales_count,
            marketing_count = EXCLUDED.marketing_count,
            design_count    = EXCLUDED.design_count,
            data_count      = EXCLUDED.data_count,
            ops_count       = EXCLUDED.ops_count,
            people_count    = EXCLUDED.people_count,
            other_count     = EXCLUDED.other_count,
            remote_count    = EXCLUDED.remote_count,
            us_count        = EXCLUDED.us_count,
            europe_count    = EXCLUDED.europe_count,
            apac_count      = EXCLUDED.apac_count,
            other_geo_count = EXCLUDED.other_geo_count
        "#,
    )
    .bind(company_id)
    .bind(date)
    .execute(db)
    .await?;

    Ok(())
}
