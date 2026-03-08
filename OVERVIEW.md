# Program Brief
A SAAS site that allows users to watch pages for changes and get notified via email.
Specifically tailored for Company career pages. 

# Tech Stack
## services:
reverse proxy: caddy
app server: rust binary
database: postgres
queue: redis (optional)

## diagram:
internet
   ↓
caddy
   ↓
rust app
   ↓
postgres

## jobs (scraping, emails etc.):
cron → rust worker

