#!/usr/bin/env bash
# Deploy Career Watch to a Linux server.
#
# Prerequisites on the server:
#   - Docker + Docker Compose v2
#   - Git
#   - Node.js (for building the frontend)
#
# Usage (from project root on the server):
#   cp .env.prod.example .env.prod   # fill in all values first
#   bash scripts/deploy.sh

set -euo pipefail

DOMAIN="${DOMAIN:-your-domain.com}"

echo "▶ Pulling latest code..."
git pull --ff-only

echo "▶ Building frontend..."
cd web
npm ci --silent
npm run build
cd ..

echo "▶ Building Docker images..."
docker compose -f docker-compose.prod.yml build --parallel

echo "▶ Starting / restarting services..."
docker compose -f docker-compose.prod.yml up -d --remove-orphans postgres
echo "  Waiting for Postgres to be healthy..."
docker compose -f docker-compose.prod.yml exec postgres \
  pg_isready -U career_watch --timeout=30 >/dev/null

docker compose -f docker-compose.prod.yml up -d --remove-orphans api caddy

echo "▶ Checking health..."
sleep 3
docker compose -f docker-compose.prod.yml ps

echo ""
echo "✓ Deployed. Service running at https://${DOMAIN}"
echo ""
echo "  Worker cron (add to crontab if not already set):"
echo "  */15 * * * * cd $(pwd) && docker compose -f docker-compose.prod.yml run --rm worker >> /var/log/career-watch-worker.log 2>&1"
echo "  0 */6 * * *  cd $(pwd) && docker compose -f docker-compose.prod.yml run --rm discovery >> /var/log/career-watch-discovery.log 2>&1"
