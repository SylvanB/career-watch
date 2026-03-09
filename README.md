## Career Watch

A SaaS application that monitors company career pages for job listing changes and notifies users by email when roles appear or disappear.

```
internet
   ↓
Caddy  (TLS termination, static files, API proxy)
   ↓
career-watch-api       (Axum HTTP API, port 8080)
   ↓
PostgreSQL

cron → career-watch-worker      (change detection + email alerts)
cron → career-watch-discovery   (automatic career page discovery)
```

---

## Quickstart (local dev)

### Prerequisites

- [Rust](https://rustup.rs) stable toolchain
- [Docker](https://docs.docker.com/get-docker/) + Docker Compose v2
- [Node.js](https://nodejs.org) 18+ (for the Astro frontend)

### 1. Start infrastructure

```sh
cp .env.example .env       # SMTP and app settings pre-configured for local dev
make up                    # starts Postgres on :5432 and Mailpit on :8025
```

### 2. Run the API

```sh
make run-api               # migrations run automatically; API on :8080
```

### 3. Run the frontend

```sh
make web-install           # first time only
make web-dev               # Astro dev server on :4321, proxies /api/* to :8080
```

Open **http://localhost:4321** — register, add watchers, trigger changes with the test server.

### 4. View caught emails

**http://localhost:8025** — Mailpit catches everything; nothing is delivered.

---

## Infrastructure (Docker Compose)

| Service  | Local address           | Purpose                                         |
|----------|-------------------------|-------------------------------------------------|
| Postgres | `localhost:5432`        | Primary database                                |
| Mailpit  | `http://localhost:8025` | Local SMTP — catches all outbound email         |

```sh
make up           # start Postgres + Mailpit (background)
make down         # stop everything
make logs         # tail container logs
make psql         # psql shell inside the container
make db-reset     # drop → recreate → migrate
make migrate      # run pending migrations only
```

The Rust API and workers run natively for faster dev iteration. To run everything in Docker (production-like), use the `app` profile:

```sh
docker compose --profile app up
```

---

## Configuration

```sh
cp .env.example .env   # local dev defaults included
```

| Variable               | Default (local dev)                                               | Description                                              |
|------------------------|-------------------------------------------------------------------|----------------------------------------------------------|
| `DATABASE_URL`         | `postgres://career_watch:password@localhost:5432/career_watch`   | PostgreSQL connection string                             |
| `BIND_ADDR`            | `0.0.0.0:8080`                                                    | API server bind address                                  |
| `SMTP_HOST`            | `localhost`                                                       | SMTP host (Mailpit locally)                              |
| `SMTP_PORT`            | `1025`                                                            | SMTP port                                                |
| `SMTP_USER`            | *(blank)*                                                         | SMTP username / API key                                  |
| `SMTP_PASS`            | *(blank)*                                                         | SMTP password / secret                                   |
| `SMTP_FROM`            | `Career Watch <no-reply@localhost>`                               | Sender address                                           |
| `SMTP_TLS`             | `false`                                                           | `false` for Mailpit; `true` for production SMTP          |
| `STRIPE_SECRET_KEY`    | *(blank)*                                                         | Stripe secret key (leave blank to skip billing in dev)   |
| `STRIPE_WEBHOOK_SECRET`| *(blank)*                                                         | Stripe webhook signing secret                            |
| `STRIPE_PRICE_PRO`     | *(blank)*                                                         | Stripe price ID for the Pro tier                         |
| `STRIPE_PRICE_BUSINESS`| *(blank)*                                                         | Stripe price ID for the Business tier                    |
| `APP_BASE_URL`         | `http://localhost:4321`                                           | **Frontend** URL — email links point here                |
| `ADMIN_SECRET`         | `change-me-in-production`                                         | Bearer token for `/admin/*` endpoints                    |
| `COOKIE_SECURE`        | `false`                                                           | Set `true` in production (requires HTTPS)                |
| `RUST_LOG`             | `info,tower_http=info`                                            | Logging filter                                           |

> **`APP_BASE_URL` must point at the frontend (Astro), not the Rust API.** Verification and password-reset links open a page — not a raw API endpoint.
>
> **Server refuses to start** if `COOKIE_SECURE=true` and `ADMIN_SECRET` is the default value.

---

## API reference

All protected endpoints require a valid session cookie (`cw_session`), set by `/auth/login` or `/auth/register`.

### Health

```
GET /healthz → 200 {"status":"ok","db":"ok"} | 503 {"status":"degraded","db":"error"}
```

### Auth  *(rate-limited: 10 req/min per IP)*

| Method | Path                       | Body                              | Response         |
|--------|----------------------------|-----------------------------------|------------------|
| POST   | `/auth/register`           | `{email, password}`               | 201 + session cookie |
| POST   | `/auth/login`              | `{email, password}`               | 200 + session cookie |
| POST   | `/auth/logout`             | —                                 | 204, clears cookie |
| POST   | `/auth/verify-email`       | `{token}`                         | 204              |
| POST   | `/auth/forgot-password`    | `{email}`                         | 204 always       |
| POST   | `/auth/reset-password`     | `{token, new_password}`           | 204              |

### Current user

```
GET /api/me → {user_id, email, email_verified, tier, max_watchers,
               check_interval_minutes, monthly_price_usd, unread_notifications}
```

### Watchers  *(requires auth, email_verified = true)*

| Method | Path                | Body                                        | Response         |
|--------|---------------------|---------------------------------------------|------------------|
| GET    | `/api/watches`      | —                                           | 200 JSON array   |
| POST   | `/api/watches`      | `{company_name, url, job_selector?}`        | 201 JSON watcher |
| DELETE | `/api/watches/:id`  | —                                           | 204 or 404       |

Returns `402 Payment Required` when the tier watcher limit is reached.

### Notifications  *(requires auth)*

| Method | Path                           | Query params                   | Response       |
|--------|--------------------------------|--------------------------------|----------------|
| GET    | `/api/notifications`           | `limit`, `offset`, `unread_only` | 200 JSON array |
| POST   | `/api/notifications/read-all`  | —                              | 204            |

### Billing  *(requires auth)*

| Method | Path                 | Body          | Response                     |
|--------|----------------------|---------------|------------------------------|
| POST   | `/billing/checkout`  | `{plan}`      | 200 `{checkout_url}`         |
| POST   | `/billing/portal`    | —             | 200 `{portal_url}`           |
| POST   | `/billing/webhook`   | raw body      | 200 (Stripe signature verified) |

`plan` is `"pro"` or `"business"`.

### Admin  *(requires `Authorization: Bearer {ADMIN_SECRET}`)*

| Method | Path                     | Response                |
|--------|--------------------------|-------------------------|
| GET    | `/admin/users`           | paginated user list     |
| GET    | `/admin/users/:id`       | user detail + watcher count |
| PATCH  | `/admin/users/:id/tier`  | 204 — override tier     |

---

## Frontend

Built with [Astro 4](https://astro.build), [Tailwind CSS](https://tailwindcss.com), and [Alpine.js](https://alpinejs.dev). Outputs static files to `web/dist/`, served by Caddy in production.

### Pages

| Route              | Description                                           |
|--------------------|-------------------------------------------------------|
| `/`                | Landing page with features and pricing                |
| `/login`           | Sign-in form                                          |
| `/signup`          | Registration with optional paid-plan Checkout flow    |
| `/dashboard`       | Watcher table, add-watcher modal, tier usage          |
| `/notifications`   | Notification archive with dark diff view              |
| `/account`         | Plan details, upgrade flow, Stripe Billing Portal     |
| `/verify-email`    | Reads `?token=` from URL and POSTs to API             |
| `/reset-password`  | Password reset form (reads `?token=` from URL)        |
| `/forgot-password` | Request a password reset link                         |
| `/terms`           | Terms of Service                                      |
| `/privacy`         | Privacy Policy                                        |

### Dev commands

```sh
make web-install   # npm install (first time)
make web-dev       # dev server on :4321, HMR, proxies /api /auth /billing
make web-build     # production build → web/dist/
```

---

## Subscription tiers

| Tier     | Watchers | Check interval | Price      |
|----------|----------|----------------|------------|
| Free     | 3        | Every 24 h     | $0/month   |
| Pro      | 25       | Every 1 h      | $9/month   |
| Business | 200      | Every 15 min   | $29/month  |

The worker cron runs every 15 minutes. The SQL `WHERE` clause filters which watchers are actually due based on the owner's tier — no separate cron entries needed.

---

## Workers

### career-watch-worker — change detection

```sh
make run-worker
# or: cargo run --bin career-watch-worker
```

Fetches active watchers due for a check (filtered by tier interval), scrapes each page, diffs job listings, sends email + archives a notification row when changes are detected.

**First run:** establishes baseline — no notification sent.
**Subsequent runs:** sends email + creates notification if hash or job list changed.

Cron (every 15 min):
```cron
*/15 * * * * cd /path/to/career-watch && ./career-watch-worker >> /var/log/cw-worker.log 2>&1
```

### career-watch-discovery — automatic discovery

```sh
make run-discovery
# or: cargo run --bin career-watch-discovery
```

1. Fetches company leads from seed sources (static list, GitHub orgs, startup directories)
2. Inserts new companies into `companies` table (deduped by domain)
3. Probes pending companies — tries `/careers`, `/jobs`, `/join-us`, and 12 other common paths
4. Detects ATS platform (Greenhouse, Lever, Workday, Ashby, SmartRecruiters, BambooHR)
5. Validates that the page contains job listings
6. Re-probes active companies with stale `last_probed_at`

Cron (every 6 h):
```cron
0 */6 * * * cd /path/to/career-watch && ./career-watch-discovery >> /var/log/cw-discovery.log 2>&1
```

---

## Test server

A fake career page server for end-to-end testing — no real companies needed.

```sh
make run-test-server   # fake career page on :9090, admin UI on /admin
```

### End-to-end walkthrough

```sh
make up && make run-api && make run-test-server   # terminals 1-3

make test-watcher   # register a watcher pointed at localhost:9090/careers
make run-worker     # first run — baseline, no notification

make test-add-job   # add "Staff Engineer" to the fake page
make run-worker     # detects the change, sends email + archives notification

open http://localhost:8025   # view the email in Mailpit
# open http://localhost:4321/notifications  → see the notification in the UI
```

Admin API:
```sh
make test-add-job          # POST /admin/jobs    — add a job
make test-rm-job           # DELETE /admin/jobs/:slug — remove a job
make test-reset            # POST /admin/reset   — restore defaults
curl http://localhost:9090/admin/jobs | jq   # list current jobs
```

---

## Database schema

Migrations in `migrations/`, applied automatically on startup.

### `watchers` — user subscriptions

| Column            | Type          | Notes                                  |
|-------------------|---------------|----------------------------------------|
| `id`              | UUID PK       |                                        |
| `user_id`         | UUID FK       | → `users.id`, nullable for legacy rows |
| `company_name`    | TEXT          |                                        |
| `url`             | TEXT          | Career page URL                        |
| `email`           | TEXT          | Notification recipient (user's email)  |
| `last_hash`       | TEXT          | SHA-256 of last fetched page           |
| `last_checked_at` | TIMESTAMPTZ   |                                        |
| `created_at`      | TIMESTAMPTZ   |                                        |
| `active`          | BOOLEAN       | Soft-delete flag                       |
| `last_jobs_json`  | JSONB         | Snapshot of last extracted jobs        |
| `job_selector`    | TEXT          | CSS selector for job-level diffs       |

### `users` — accounts

| Column                  | Type                 | Notes                                       |
|-------------------------|----------------------|---------------------------------------------|
| `id`                    | UUID PK              |                                             |
| `email`                 | TEXT UNIQUE          |                                             |
| `password_hash`         | TEXT                 | Argon2id                                    |
| `email_verified`        | BOOLEAN              | Required before creating watchers           |
| `tier`                  | subscription_tier    | `free` \| `pro` \| `business`               |
| `stripe_customer_id`    | TEXT UNIQUE          |                                             |
| `stripe_subscription_id`| TEXT UNIQUE          |                                             |
| `subscription_status`   | TEXT                 | `inactive` \| `active` \| `past_due` \| `cancelled` |
| `is_admin`              | BOOLEAN              |                                             |
| `created_at`            | TIMESTAMPTZ          |                                             |
| `updated_at`            | TIMESTAMPTZ          |                                             |

### `sessions`

| Column       | Type        | Notes                                     |
|--------------|-------------|-------------------------------------------|
| `id`         | UUID PK     |                                           |
| `user_id`    | UUID FK     |                                           |
| `token_hash` | TEXT UNIQUE | SHA-256 of the cookie value               |
| `created_at` | TIMESTAMPTZ |                                           |
| `expires_at` | TIMESTAMPTZ | Default: 30 days                          |

### `email_tokens` — verification & password reset

| Column       | Type        | Notes                                          |
|--------------|-------------|------------------------------------------------|
| `id`         | UUID PK     |                                                |
| `user_id`    | UUID FK     |                                                |
| `token_hash` | TEXT UNIQUE | SHA-256 of the link token                      |
| `kind`       | TEXT        | `email_verification` \| `password_reset`       |
| `expires_at` | TIMESTAMPTZ | 24 h for verification, 1 h for password reset  |
| `used_at`    | TIMESTAMPTZ | Set atomically on first use; prevents replay   |

### `notifications` — alert archive

| Column        | Type        | Notes                              |
|---------------|-------------|------------------------------------|
| `id`          | UUID PK     |                                    |
| `user_id`     | UUID FK     |                                    |
| `watcher_id`  | UUID FK     | SET NULL if watcher deleted        |
| `company_name`| TEXT        |                                    |
| `career_url`  | TEXT        |                                    |
| `summary`     | TEXT        | e.g. "3 jobs added, 1 removed"     |
| `added_jobs`  | JSONB       | Array of `JobPosting`              |
| `removed_jobs`| JSONB       | Array of `JobPosting`              |
| `read_at`     | TIMESTAMPTZ | NULL = unread                      |
| `created_at`  | TIMESTAMPTZ |                                    |

### `companies` — discovery pipeline

| Column          | Type          | Notes                                    |
|-----------------|---------------|------------------------------------------|
| `id`            | UUID PK       |                                          |
| `name`          | TEXT          |                                          |
| `domain`        | TEXT UNIQUE   | e.g. `stripe.com`                        |
| `source`        | TEXT          | Where this lead came from                |
| `career_url`    | TEXT          | Resolved career page after probing       |
| `platform`      | TEXT          | `greenhouse` \| `lever` \| `workday` …   |
| `platform_id`   | TEXT          | Company slug on the ATS                  |
| `status`        | TEXT          | `pending` → `active` \| `failed`         |
| `probe_error`   | TEXT          |                                          |
| `created_at`    | TIMESTAMPTZ   |                                          |
| `last_probed_at`| TIMESTAMPTZ   |                                          |

### `stripe_events` — idempotency log

| Column           | Type        | Notes                                  |
|------------------|-------------|----------------------------------------|
| `stripe_event_id`| TEXT PK     | Stripe's event ID; prevents re-processing |
| `event_type`     | TEXT        |                                        |
| `processed_at`   | TIMESTAMPTZ |                                        |
| `payload`        | JSONB       |                                        |

---

## Security model

- **Passwords** — Argon2id with a random salt; never stored in plaintext
- **Session tokens** — 32 random bytes; only the SHA-256 hash is stored in the DB. A stolen database dump cannot be used to hijack sessions
- **Email tokens** — same hash-only approach; marked used atomically to prevent replay
- **Rate limiting** — 10 requests per 60 seconds per IP on all `/auth/*` endpoints; returns `429`
- **Admin API** — protected by `Authorization: Bearer {ADMIN_SECRET}`; server refuses to start with the default secret when `COOKIE_SECURE=true`
- **Stripe webhooks** — HMAC-SHA256 signature verification on raw request bytes; idempotent via `stripe_events` table

---

## Production deployment

### 1. Configure the server

```sh
git clone https://github.com/you/career-watch /srv/career-watch
cd /srv/career-watch

cp .env.prod.example .env.prod
# Edit .env.prod — fill in every CHANGE_ME value
# Generate a secret: openssl rand -hex 32

# Set your domain in Caddyfile.prod
sed -i 's/your-domain.com/careerwatch.io/g' Caddyfile.prod
```

### 2. Deploy

```sh
bash scripts/deploy.sh
```

This runs: `git pull` → `npm build` → `docker compose build` → `docker compose up`.

### 3. Configure cron

```cron
# Check for job changes every 15 minutes
*/15 * * * * cd /srv/career-watch && docker compose -f docker-compose.prod.yml run --rm worker >> /var/log/cw-worker.log 2>&1

# Discover new career pages every 6 hours
0 */6 * * * cd /srv/career-watch && docker compose -f docker-compose.prod.yml run --rm discovery >> /var/log/cw-discovery.log 2>&1
```

### 4. Set up Stripe

1. Create two products in the [Stripe Dashboard](https://dashboard.stripe.com/products): **Pro** ($9/mo) and **Business** ($29/mo)
2. Copy the price IDs (`price_...`) into `.env.prod`
3. Add a webhook endpoint at `https://your-domain.com/billing/webhook` for events: `customer.subscription.created`, `customer.subscription.updated`, `customer.subscription.deleted`, `invoice.payment_failed`, `invoice.payment_succeeded`
4. Copy the webhook signing secret (`whsec_...`) into `.env.prod`

### Re-deploy

```sh
git pull && bash scripts/deploy.sh
```

---

## Project layout

```
src/
  main.rs                    API server entrypoint
  lib.rs                     Shared library — AppState, module exports
  config.rs                  Environment variable loading
  models.rs                  DB + API data models; JobPosting identity rules
  db.rs                      PostgreSQL connection pool
  scraper.rs                 HTTP fetching, SHA-256 hashing, CSS job extraction
  email.rs                   SMTP client — all transactional email templates
  rate_limit.rs              IP-based sliding window rate limiter
  auth/
    mod.rs
    password.rs              Argon2id hash + verify
    session.rs               Token generation, DB sessions, email tokens
    middleware.rs            require_auth and require_admin Axum middleware
  billing/
    mod.rs
    stripe_client.rs         Minimal Stripe REST client (reqwest); webhook sig verify
    webhooks.rs              Stripe event dispatch + idempotency
  discovery/
    mod.rs
    platform.rs              ATS platform detection (URL + HTML signatures)
    validator.rs             Job listing page validation
    prober.rs                Career page path prober
    sources.rs               SeedSource trait + StaticList, GithubOrg implementations
    scheduler.rs             Discovery queue, dedup, bounded-concurrency probing
  routes/
    mod.rs                   Router composition — public / protected / admin
    health.rs                GET /healthz
    auth.rs                  Auth handlers
    watches.rs               Watcher CRUD (tenant-scoped, tier-limited)
    notifications.rs         Notification list + mark-read
    billing.rs               Stripe Checkout, Portal, Webhook
    admin.rs                 Admin user management
    me.rs                    GET /api/me
  bin/
    worker.rs                career-watch-worker
    discovery_worker.rs      career-watch-discovery
    test_server.rs           career-watch-test-server (local E2E)

web/                         Astro frontend
  src/
    layouts/
      Base.astro             HTML shell, Inter font, Alpine.js CDN
      App.astro              Authenticated shell — auth check, nav
    pages/
      index.astro            Landing page
      login.astro / signup.astro
      dashboard.astro        Watcher management
      notifications.astro    Alert archive with dark diff view
      account.astro          Plan + billing
      verify-email.astro / reset-password.astro / forgot-password.astro
      terms.astro / privacy.astro / 404.astro
    styles/
      global.css             Tailwind component classes
  astro.config.mjs           Vite proxy config for dev
  tailwind.config.mjs

migrations/
  0001_init.sql              watchers table
  0002_jobs_snapshot.sql     last_jobs_json, job_selector
  0003_discovery.sql         companies table
  0004_users_sessions.sql    users, sessions, email_tokens
  0005_watchers_user_id.sql  watchers.user_id FK
  0006_stripe_events.sql     stripe_events idempotency table
  0007_notifications.sql     notifications archive

scripts/
  db-reset.sh                Drop → recreate → migrate
  deploy.sh                  Production deploy script

docker-compose.yml           Local dev: Postgres + Mailpit
docker-compose.prod.yml      Production: all services + Caddy
Caddyfile                    Local dev reverse proxy config
Caddyfile.prod               Production Caddy config (TLS, security headers)
.env.example                 Local dev environment template
.env.prod.example            Production environment template
Makefile                     Development task runner
Dockerfile                   Multi-stage Rust build (api / worker / discovery)
```

---

## Testing

```sh
cargo test        # 72 unit tests, ~0.9 s
make test         # alias
```

### Test coverage

| Module | Tests | Covers |
|--------|-------|--------|
| `models` | 9 | `JobPosting` identity (eq/hash ignore metadata), tier watcher limits, check intervals, pricing |
| `auth::session` | 6 | Token length, uniqueness, hash determinism, raw ≠ hash |
| `auth::password` | 1 | Argon2id round-trip, wrong-password rejection |
| `rate_limit` | 6 | Under limit, over limit, per-IP isolation, per-path isolation, window eviction, burst |
| `billing::stripe_client` | 2 | Webhook sig rejection (bad sig, empty header) |
| `billing::webhooks` | 5 | `tier_for_price` — pro, business, unknown, empty config, collision |
| `discovery::platform` | 14 | All ATS platforms by URL + HTML, ID extraction, board URL generation, priority rules |
| `discovery::validator` | 9 | No-openings phrases, platform shortcuts, structural markers, empty page, density heuristic |
| `discovery::sources` | 4 | Domain extraction, www stripping, empty input |
| `scraper` | 3 | Hash determinism, hash sensitivity, CSS job extraction |
| `config` | 2 | Required keys, BIND_ADDR default |
| `bin/worker` | 12 | `diff_jobs` (add/remove/empty/identical/metadata-only), `notification_summary` (all 5 cases) |
