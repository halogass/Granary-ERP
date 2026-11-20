# Troubleshooting

<!-- nav:top -->
[Home](Home) › Administration › **Troubleshooting**

**On this page:** [Quick health check](#quick-health-check) · [Starting up](#starting-up) · [Signing in](#signing-in) · [The web pages](#the-web-pages) · [Settings and business errors](#settings-and-business-errors) · [Mail and messages](#mail-and-messages) · [Background jobs](#background-jobs) · [Nary and AI](#nary-and-ai) · [Face clock-in](#face-clock-in)
<!-- /nav:top -->

Common problems and how to fix them, grouped by where they show up. Most fixes need access to the server (run
the commands in the Granary folder); the ones about settings need the Owner or a system admin. If your problem is
not here, check the logs first: `docker compose logs --tail 100 backend worker`.

---

## Quick health check

```bash
docker compose ps                  # every service "running"; backend, db, redis, storage "(healthy)"
curl http://localhost/health       # through Nginx: {"status":"ok","service":"granary"}
curl http://localhost:8000/health  # straight to the backend
docker stats --no-stream           # memory and CPU
```

---

## Starting up

### Compose refuses to start: "set REDIS_PASSWORD" (or NEXTAUTH_SECRET, SIGNAL_TICKET_KEY)

`docker-compose.yml` requires these three. Add each to `.env` with its own random value (`openssl rand -hex 32`),
and put the Redis password into `REDIS_URL`, `CELERY_BROKER_URL` and `CELERY_RESULT_BACKEND`
(`redis://:<password>@redis:6379/0`, `/1`, `/2`).

### The backend exits at start because of a placeholder secret

The API and the worker refuse to start while `JWT_SECRET_KEY`, `APP_SECRET_KEY` or `NEXTAUTH_SECRET` still begins
with the `.env.example` placeholder (`change-this`). The log names the variable. Generate a value for each
(`openssl rand -hex 64`, a different one per variable), put it in `.env` and start again.

Other weak secrets don't stop the start: they appear in the log as an error and the Owner gets an alert.

### The backend exits at start: "invalid configuration"

A required variable is missing or invalid; the log names it. Required: `APP_SECRET_KEY`, `DATABASE_URL`,
`POSTGRES_PASSWORD`, `JWT_SECRET_KEY` (at least 32 characters), `SUPERADMIN_PASSWORD`, `MINIO_SECRET_KEY`.
`CORS_ORIGINS` and `TURN_URIS` must be JSON lists.

### "database connection refused"

```bash
docker compose ps db
docker compose logs db
```

If the database is unhealthy, check that `POSTGRES_PASSWORD` matches the password inside `DATABASE_URL`.

### The backend restarts in a loop after an upgrade

A migration failed (it rolls back as a whole), so the API never starts. A migration that adds a unique index
aborts when existing rows break it, and the error lists them. Resolve them with the
[duplicate checks](Backups-and-Upgrades#duplicate-checks), then:

```bash
docker compose restart backend
docker compose exec -T backend granary-migrate --status | tail -3
```

### Port 80 already in use

```bash
sudo lsof -i :80
sudo systemctl stop apache2 && sudo systemctl disable apache2   # often Apache
sudo systemctl reload nginx
```

### The Cloudflare Tunnel is offline

```bash
sudo systemctl status cloudflared
sudo journalctl -u cloudflared -n 50
sudo systemctl restart cloudflared
```

---

## Signing in

### Users are signed out after every restart

`NEXTAUTH_SECRET` or `JWT_SECRET_KEY` changed. Keep both fixed in `.env`, then
`docker compose up -d frontend backend`.

### Sign-in fails (401 on `/api/auth/callback/credentials`)

1. The frontend can't reach the backend: `INTERNAL_API_URL` must be `http://backend:8000`.
2. A database error at sign-in: `docker compose logs backend | grep -iE "error|login"`.
3. A rate limit: Nginx allows 5 sign-in requests per minute per address (burst 10), and the backend allows
   `auth.login_max_attempts` failed sign-ins (default 5) per `auth.login_window_seconds` (default 900) per address,
   then answers "Too many login attempts". People behind one office address share the limit.

### Signed in, but every API call returns 401

The access token was not refreshed. Check that the frontend reaches the backend:

```bash
docker compose exec frontend sh -c "wget -qO- http://backend:8000/health"
docker compose logs backend | grep -iE "refresh|401|error"
```

### Someone lost their authenticator phone

They sign in with a recovery code instead of the app code. If they have none left, a system admin opens
**Settings > Users**, expands the user and resets **Two-factor sign-in**; the person sets it up again in
**My profile**.

### The sign-in rate limiter isn't limiting

It fails open on purpose when Redis is down or its settings are missing, and logs
`login rate limit skipped` or `login rate limit unavailable`. Set **Failed logins allowed** and **Login limit
window** in **Settings > System & security > Login security**, or fix Redis. Nginx's own limit still applies.

---

## The web pages

### Mixed Content warnings, or calls go to `http://`

`INTERNAL_API_URL` or `API_URL` is set to the public domain. Set both to `http://backend:8000`, then
`docker compose up -d --build frontend`.

### Blank page, or calls go to the wrong host

`API_URL` and `SIGNAL_URL` are baked into the frontend at build time. After changing them:
`docker compose up -d --build frontend`.

### Uploads fail with a storage error

Check that the storage is healthy (`docker compose ps storage`) and that `MINIO_ACCESS_KEY` and
`MINIO_SECRET_KEY` in `.env` are the ones the `storage` service started with. It reads them when it starts, so
run `docker compose up -d storage` after a change. Buckets need no setup: Granary creates each one on first use.

### Links to files don't open

Files reach the browser at `MINIO_PUBLIC_URL` (in production `https://<your domain>/minio`, proxied read-only by
Nginx). Check that the variable matches your domain and that the live Nginx file has the `location /minio/` block.

---

## Settings and business errors

### "Setting 'x' is not set" (400)

Every business number is a setting, and Granary refuses to guess when one is blank or malformed. The message
names the setting, for example `Setting 'projects.default_hourly_cost' is not set.` Variants: `is not a number`,
`is not valid JSON`, `must be a whole number`, `can't be negative`, `has no entry for USD`.

Fix: type the key into the Settings search box and enter a valid value (Owner or system admin). Frequent cases:

| Setting | When it bites |
|---|---|
| `projects.default_hourly_cost` | Logging time for a user with no employee record |
| `projects.monthly_hours` | Logging time; must not be 0 |
| `payroll.*` | Creating or calculating a payroll run |
| `currency.decimals` | Any money action in a currency missing from it |
| `timezone` | Anything that needs a business date; must be a valid zone name such as `Asia/Tokyo` |
| `finance.gl_autopost_from` | Must be blank or `YYYY-MM-DD` |

If a key is missing entirely, restart the backend: the seed adds every missing default on start.

### "No account is mapped to the 'x' role"

Automatic ledger posting is on and needs an account role that has no active account. Map it in
**Finance > Chart of Accounts** (account mapping), then retry: the action did nothing. Related:
`Bank account 'X' has no GL account` (set it on the bank account in **Finance > Bank Reconciliation**), and the
depreciation accounts on an asset category (**Finance > Asset Categories**). See [Finance](Finance).

### "No USD/JPY exchange rate is on record for ..."

There is no rate for that pair on or before the date. Granary never uses 1.0 instead. Add the rate in
**Finance > Exchange Rates** (or **Sync now** for today's rates), or type the rate on the document. See
[Multi-Currency](Multi-Currency).

### FX rates are not syncing

1. No API key: set `EXCHANGERATE_API_KEY` in `.env` or save a key in **Settings > Money & tax > Currencies**.
   The worker logs `No ExchangeRate API key configured`.
2. `beat` is not running: `docker compose ps beat`, `docker compose restart beat`.
3. The key is invalid: use **Test** in the Currencies settings, or look for `ExchangeRate-API returned 401` in the
   worker log.

### "Fiscal period ... is locked"

The date falls in a locked period. Date the document in an open period, or have finance unlock the period in
**Finance > Fiscal Periods** (permission `finance:post_journal`).

### Payroll: "A payroll run for MM/YYYY already exists"

Only one non-cancelled run per month is allowed. Open the existing run in **HR > Payroll**, or cancel it first.

---

## Mail and messages

### Sending a quotation or invoice fails or says "already sent"

| Message | Meaning |
|---|---|
| "Submitted for approval ..." | Nothing was sent; it goes out once approved |
| "was already sent to ... Send it again?" | Same document, same recipient, in the last 15 minutes. Click **Send again** only if you mean it |
| "is already being sent to ..." | An identical send is still running |
| "The document could not be rendered" | The `pdf` service is down or slow: `docker compose logs --tail 50 pdf`. Nothing was sent |
| "The CRM inbox ... is not configured" | Set up the inbox in **Settings > Email & WhatsApp > CRM inboxes** |

If sends time out at exactly 60 seconds, the live Nginx file lacks the `location /api/v1/documents/` block
(180 s timeout). Add that block by hand above `location /api/`.

### Email campaigns are not sending

1. The daily limit (`marketing.email_daily_limit`, default 100) is reached. Sending resumes after midnight,
   company time.
2. The campaign settings are inconsistent (the backend log shows `Email queue worker not configured`): the lock TTL
   must be longer than the max delay, and the max delay not below the min. Fix them in
   **Settings > Email & WhatsApp > Email campaigns**.
3. Redis is down or a lock is stuck; it expires after `marketing.queue_lock_ttl_seconds`.
4. The CRM inbox's SMTP is not configured or its password is wrong. Use **Send test** in the inbox settings.

### Scheduled messages are not going out

The job runs every 5 minutes; if its settings are missing it logs `scheduled_messages_not_configured`. A message
interrupted while sending is marked failed after `crm.scheduled.stuck_after_minutes` ("Interrupted while sending;
check the chat before resending"), never sent twice. WhatsApp messages whose author lost `crm:send_whatsapp` are
not sent.

### A personal mailbox is not syncing

IMAP errors (wrong password, server unreachable) are shown on the account in **Email**. Each folder is synced by
one process at a time; a lock left by a crashed worker expires after `mail.sync_lock_ttl_seconds` (default 600).

### WhatsApp shows "not connected"

Granary does not ship a WhatsApp Web bridge. Without a service at `WHATSAPP_SERVICE_URL` (token
`WHATSAPP_SERVICE_TOKEN`), "not connected" is expected and the rest of Granary works.

---

## Background jobs

```bash
docker compose ps worker beat
docker compose logs worker
docker compose logs beat
docker compose up -d backend worker beat     # after an upgrade, recreate all three together
```

Jobs that run but skip with "not configured":

| Log event | Settings |
|---|---|
| `invoice_reminders_not_configured` | `finance.invoice_reminder_days`, `finance.invoice_reminder_repeat_days` |
| `scheduled_messages_not_configured` | `crm.scheduled.max_attempts`, `crm.scheduled.stuck_after_minutes` |
| `signature_reminders_not_configured` | `sign.reminder_window_hours`, `sign.reminder_interval_hours` |
| `mail_sync_not_configured` | `mail.sync_lock_ttl_seconds` |

Fix the setting; the next run picks it up without a restart.

---

## Nary and AI

### Nary answers with an error

| Message | Fix |
|---|---|
| "The AI provider rejected the API key" | Enter the key again in **Settings > Nary & AI > Providers & keys** |
| "Model '...' was not found on this provider" | Pick a model the provider offers in **Model per job** |
| "rate limiting us or the account is out of credit" | Wait, or top up the provider account |
| "The AI provider didn't answer in time" | The provider is slow or unreachable |
| "No API key for ..., which the '...' job uses" | Save that provider's key, or route the job to a provider you have a key for |

The shipped routing uses Anthropic for most jobs and Gemini for embeddings and web search. With keys for other
providers only, change the routing first.

### Nary ignores your SOPs or AI knowledge

The embedding job has no working key or model. Route embeddings to a provider with an embeddings API (DeepSeek
has none), then re-index from **Settings > Nary & AI**.

### CRM enrichment or research does nothing

Both need a web-search backend: **Settings > Nary & AI > Web research**. For SearXNG, check
`docker compose ps searxng research`.

### An AI client can't apply an action

At full power, sensitive actions need your **Allow** click on `nary_apply` and, with
`mcp.apply.require_totp` on and two-factor sign-in on, the current code from your app, typed by you. A prepared
action expires after `mcp.owner.confirm_minutes`; ask the client to prepare it again. See
[MCP for AI Clients](MCP-for-AI-Clients).

---

## Face clock-in

```bash
docker compose ps face
docker compose logs --tail 50 face
```

- Matches too strict or too loose: adjust **Face match strictness** in **Settings > People & payroll >
  Attendance** (default 0.50).
- A kiosk says it was revoked or is not registered: sign in as HR on the tablet at `/clock?kiosk=1` again.
- "Location required": allow location for the site in the browser, then **Reload after allowing**.

See [Face Clock-In](Face-Clock-In).

---

## Related pages

- [Backups and Upgrades](Backups-and-Upgrades)
- [Configuration](Configuration)
- [FAQ](FAQ)

<!-- nav:bottom -->
---

← Previous: [Backups and Upgrades](Backups-and-Upgrades) | Next: [Developer Guide](Developer-Guide) →
<!-- /nav:bottom -->
