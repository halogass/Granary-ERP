# Troubleshooting

---

## Compose refuses to start: "set REDIS_PASSWORD" (or NEXTAUTH_SECRET, SIGNAL_TICKET_KEY)

`docker-compose.yml` requires these three variables. Add each to `.env` with its own random value
(`openssl rand -hex 32`), and put the Redis password into `REDIS_URL`, `CELERY_BROKER_URL` and
`CELERY_RESULT_BACKEND` (`redis://:<password>@redis:6379/0`, `/1`, `/2`).

---

## The backend exits at start: "invalid configuration"

`granary-api` and `granary-worker` refuse to start when a required variable is missing or invalid; the log
names it. Required: `APP_SECRET_KEY`, `DATABASE_URL`, `POSTGRES_PASSWORD`, `JWT_SECRET_KEY` (at least 32
characters), `SUPERADMIN_PASSWORD`, `MINIO_SECRET_KEY`. `CORS_ORIGINS` and `TURN_URIS` must be JSON lists.

```bash
docker compose logs --tail 50 backend
```

---

## Users are signed out after every restart

**Cause**: `NEXTAUTH_SECRET` (the frontend's session key) or `JWT_SECRET_KEY` (the backend's token key) changed
between restarts, which invalidates every session.

**Fix**: keep both set to permanent values in `.env`, then restart:

```bash
docker compose up -d frontend backend
```

---

## Sign-in succeeds but every API call returns 401

**Cause**: the backend access token (60 minutes) expired and was not refreshed. The NextAuth `jwt()` callback
normally refreshes it through `POST /api/v1/auth/refresh` before it expires.

```bash
# Can the frontend reach the backend?
docker compose exec frontend sh -c "wget -qO- http://backend:8000/health"
# Expected: {"status":"ok","service":"granary"}

docker compose logs backend | grep -iE "refresh|401|error"
```

---

## Sign-in fails with 401 on `/api/auth/callback/credentials`

NextAuth's `authorize()` returns nothing when the backend sign-in call fails. Common reasons:

1. The backend is not reachable from the frontend container: `INTERNAL_API_URL` is wrong.
2. A database error at sign-in time.
3. A rate limit: Nginx allows 5 requests per minute per address on `/api/v1/auth/login` (burst 10), and the
   backend allows `auth.login_max_attempts` (default 5) failed sign-ins per `auth.login_window_seconds` (default
   900) per address before answering 429 "Too many login attempts". People behind one office address share the
   limit.

```bash
docker compose logs backend | grep -iE "error|login"
docker compose ps db   # should show (healthy)
```

---

## API calls fail with a Mixed Content warning (http:// instead of https://)

**Cause**: `INTERNAL_API_URL` or `API_URL` is set to the public HTTPS domain. The Next.js rewrite uses it as the
proxy destination, Nginx redirects `http://` to `https://`, and the redirect leaks to the browser.

**Fix**: point both at the Docker-internal backend, then restart (a frontend built from source with the public
domain needs `docker compose build frontend` too):

```dotenv
INTERNAL_API_URL=http://backend:8000
API_URL=http://backend:8000
```

```bash
docker compose up -d backend worker beat frontend
```

---

## The backend won't start: "database connection refused"

```bash
docker compose logs db
docker compose ps db
# If "unhealthy": check that POSTGRES_PASSWORD matches the password in DATABASE_URL
```

---

## The frontend shows a blank page, or calls go to the wrong host

The browser calls the address it was opened on, so check that Nginx passes `/api/` and `/ws/signal` on. A
frontend built from source with `NEXT_PUBLIC_API_URL` or `NEXT_PUBLIC_SIGNAL_URL` set uses those instead;
rebuild it without them: `docker compose build frontend && docker compose up -d frontend`.

---

## WhatsApp shows "not connected"

Granary does not ship a WhatsApp Web bridge. The WhatsApp inbox, sending documents over WhatsApp and Nary's
WhatsApp channel need an HTTP service at `WHATSAPP_SERVICE_URL` with the token `WHATSAPP_SERVICE_TOKEN`.
Without one, "not connected" is expected and the rest of Granary is unaffected. If you run your own bridge,
check that the backend can reach it and that both sides use the same token.

---

## Face clock-in fails or doesn't recognise people

Face matching runs on the self-hosted `face` service.

```bash
docker compose ps face
docker compose logs --tail 50 face
```

- Matches too strict or too loose: adjust **Face match strictness** in **Settings → People & payroll →
  Attendance** (default 0.50).
- A kiosk tablet says "This kiosk was revoked or its session ended. Register it again." (or "This device is not
  registered as a kiosk"): sign in as HR on the tablet at `/clock?kiosk=1` again.

### "Location required" blocking screen

The browser needs location permission:

1. Click the lock or info icon in the address bar.
2. Site settings → Location → Allow.
3. Click **Reload after allowing** on the clock page.

---

## Port 80 already in use

```bash
sudo lsof -i :80
# Often Apache is running:
sudo systemctl stop apache2 && sudo systemctl disable apache2
sudo systemctl reload nginx
```

---

## The Cloudflare Tunnel is offline

```bash
sudo systemctl status cloudflared
sudo journalctl -u cloudflared -n 50
sudo systemctl restart cloudflared
cloudflared tunnel info granary
```

---

## Uploads fail with a storage error

Check that the storage is healthy (`docker compose ps storage`) and that `MINIO_ACCESS_KEY` / `MINIO_SECRET_KEY` in
`.env` are the ones the `storage` service started with (they are read when it starts: `docker compose up -d storage`
after a change). Buckets need no setup: Granary creates each one on first use.

---

## Background jobs not running

```bash
docker compose ps worker beat
docker compose logs worker      # connection errors
docker compose logs beat        # scheduler errors
docker compose restart worker beat
```

`worker`, `beat` and `backend` run the same image. After an upgrade, recreate all three
(`docker compose up -d backend worker beat`) so a beat never schedules a job the worker doesn't know.

### Jobs run but skip with "not configured"

A scheduled job whose settings are missing or malformed logs an error and returns without doing anything:

| Log event | Job | Settings |
|---|---|---|
| `invoice_reminders_not_configured` | `send_invoice_reminders` | `finance.invoice_reminder_days`, `finance.invoice_reminder_repeat_days` |
| `scheduled_messages_not_configured` | `send_scheduled_messages` | `crm.scheduled.max_attempts`, `crm.scheduled.stuck_after_minutes` |
| `signature_reminders_not_configured` | `remind_expiring_signatures` | `sign.reminder_window_hours`, `sign.reminder_interval_hours` |
| `mail_sync_not_configured` | `sync_personal_mailboxes` | `mail.sync_lock_ttl_seconds` |

```bash
docker compose logs --since 24h worker | grep -E "not_configured|failed"
```

The `error` field says which setting and where to set it. Fix it in **Settings**; the next run picks it up
without a restart.

---

## Check the health of all services

```bash
docker compose ps
curl http://localhost/health       # through Nginx
curl http://localhost:8000/health  # straight to the backend, on the server
docker stats --no-stream           # memory and CPU snapshot
```

---

## FX rates not syncing, or `exchange_rates` is empty

**Cause 1**: no API key. The sync uses `EXCHANGERATE_API_KEY` from `.env`, or the key saved in
**Settings → Money & tax → Currencies** when that is empty. With neither, the worker logs
`No ExchangeRate API key configured` and skips the sync.

```bash
grep EXCHANGERATE_API_KEY .env
# Or save the key in Settings, then Sync now. After changing .env:
docker compose up -d backend worker beat
```

**Cause 2**: `beat` is not running.

```bash
docker compose ps beat
docker compose logs beat | tail -30
docker compose restart beat
```

**Cause 3**: the key is invalid. Use **Test connection** in the Currencies settings, or look in the worker log for
`ExchangeRate-API returned 401` / `403` or `ExchangeRate-API error`.

---

## Email campaigns are not sending

The sender is a loop inside the **backend**, and only the process holding the Redis lease
`lock:marketing.email_queue_worker` sends.

**Cause 1**: the daily limit (`marketing.email_daily_limit`, default 100) is reached. Sending resumes after
midnight in the company time zone.

**Cause 2**: the campaign settings are missing or inconsistent. The backend log shows `Email queue worker not
configured` with the reason, for example `marketing.queue_lock_ttl_seconds` must be longer than
`marketing.email_max_delay_seconds`, or the max delay is below the min. Fix them in **Settings → Email &
WhatsApp → Email campaigns**. The `timezone` setting must also be valid.

**Cause 3**: Redis is down or the lease is stuck. A lease left by a killed process expires after
`marketing.queue_lock_ttl_seconds` (default 180 s).

```bash
docker compose ps redis
docker compose exec redis redis-cli TTL lock:marketing.email_queue_worker   # -2 = free
docker compose logs backend | grep -iE "queue worker|Marketing email"
```

**Cause 4**: the CRM inbox's SMTP account is not configured or its password is wrong (`Marketing email not sent:
CRM SMTP is not configured`). Check it in **Settings → Email & WhatsApp → CRM inboxes** and send a test.
Per-email failures are stored on the queue item (`email_queue_items.error_message`).

---

## Scheduled messages not going out

The job `send_scheduled_messages` runs every 5 minutes. If its settings are missing it logs
`scheduled_messages_not_configured` and sends nothing.

```bash
docker compose logs worker | grep -i scheduled
```

A message left in `sending` by an interrupted run is marked `failed` after `crm.scheduled.stuck_after_minutes`
("Interrupted while sending; check the chat before resending"). Check the conversation before scheduling it
again. WhatsApp messages whose author no longer has `crm:send_whatsapp` are not sent. Pending messages:

```bash
docker compose exec db psql -U granary granary \
  -c "SELECT id, channel, status, scheduled_at, retry_count, last_error FROM scheduled_messages WHERE status='pending' ORDER BY scheduled_at;"
```

---

## The AI assistant answers with an error

Provider problems reach the user in plain words:

| Message | Fix |
|---|---|
| "The AI provider rejected the API key" | Enter the key again in **Settings → Nary & AI → Providers & keys** |
| "Model '...' was not found on this provider" | Pick a model the provider offers, in **Model per job** |
| "rate limiting us or the account is out of credit" | Wait, or top up the provider account |
| "The AI provider didn't answer in time" | The provider is slow or unreachable |
| "No API key for ..., which the '...' job uses" | Save that provider's key, or route the job to a provider you have a key for (**Model per job**) |

The shipped routing uses Anthropic models for most jobs and Gemini for embeddings and web search. With keys for
other providers only, change the routing first.

**SOP answers ignore your SOPs**, or AI knowledge never matches: the embedding job has no working key or model.
Route embeddings to a provider with an embeddings API (DeepSeek has none), then re-index from
**Settings → Nary & AI**.

**Confirm cards**: a quotation, invoice, SOP or CRM change prepared by the assistant is saved only when you
confirm the card. After a page reload the card shows whether it was applied or discarded.

---

## CRM enrichment or deep research does nothing

Both need a web-search backend: **Settings → Nary & AI → Web research**. For SearXNG, check that the services are
up:

```bash
docker compose ps searxng research
docker compose logs --tail 30 research
```

---

## Sending a quotation or invoice fails or says "already sent"

| Message | Meaning |
|---|---|
| "Submitted for approval ..." | Nothing was sent; it goes out once approved |
| "was already sent to ... Send it again?" | The same document went to the same recipient in the last 15 minutes. Click **Send again** only if you mean it |
| "is already being sent to ..." | An identical send is still running. Wait, then check the deal's activity |
| "The document could not be rendered" | The `pdf` container is down or slow: `docker compose ps pdf` and `docker compose logs --tail 50 pdf`. Nothing was sent |
| "The CRM inbox ... is not configured" | Set up the inbox in **Settings → Email & WhatsApp → CRM inboxes** |

If sends time out at exactly 60 s, the live Nginx file lacks the `location /api/v1/documents/` block (180 s
read timeout) from `infra/nginx/granary.conf`. Add that block by hand above `location /api/` (don't copy the
whole repository file over the live one: certbot's HTTPS lines live only there), then
`sudo nginx -t && sudo systemctl reload nginx`.

---

## "Setting 'x' is not set" (400)

**Symptom**: an action fails with a message like `Setting 'projects.default_hourly_cost' is not set. Set it in
Settings → Operations → Projects.` Variants: `is not a number`, `is not valid JSON`, `must be a whole number`,
`can't be negative`, `has no entry for USD` (for `currency.decimals`).

**Cause**: every business number (rates, caps, limits, schedules, currency decimals) is a system setting, and the
code refuses to guess when one is blank or malformed. The message names the setting. The location it names may
be an older label: type the key into the Settings search box to find it.

**Fix**: enter a valid value (superadmin or system admin). The [settings reference](CONFIGURATION.md#17-system-settings-reference)
lists the common keys with their defaults. Frequent cases:

| Setting | When it bites |
|---|---|
| `projects.default_hourly_cost` | Logging time for a user with no employee record (blank by default) |
| `projects.monthly_hours` | Logging time; must not be 0 |
| `payroll.*` | Creating or calculating a payroll run |
| `currency.decimals` | Any money action in a currency missing from it, after activating a new currency |
| `timezone` | Anything that needs a business date; must be a valid zone name such as `Asia/Tokyo` |
| `finance.gl_autopost_from` | Must be blank or `YYYY-MM-DD` |

If a key is missing entirely (not just blank), restart the backend: the seed inserts every missing default on
start.

---

## "No account is mapped to the 'x' role"

**Symptom**: sending an invoice, recording a payment, approving a vendor invoice or expense, paying payroll, or
disposing an asset fails with `No account is mapped to the 'sales_tax_input' role, so this can't be posted to the
ledger. ...`, or a mapped account is inactive.

**Cause**: automatic GL posting is on (`finance.gl_autopost_from` is set and the document is dated on or after
it), and the posting needs a role that has no active account.

**Fix**: **Finance → Chart of Accounts → Account mapping**: map the role (or reactivate the account). The action
did nothing, so retry it after mapping. Related messages:

- `Bank account 'X' has no GL account.`: the chosen bank account isn't linked to a GL account; set it on the bank
  account in **Finance → Bank**.
- `Set the depreciation and accumulated depreciation accounts on the asset's category first`: set them in
  **Finance → Assets → Categories**.

To turn automatic posting off, clear `finance.gl_autopost_from` (**Settings → Money & tax → Accounting**).

---

## "No USD/JPY exchange rate is on record for ..."

**Symptom**: a 400 such as `No USD/JPY exchange rate is on record for 2026-10-09.` when converting a quotation,
recording a payment, logging time on a foreign-currency project, running payroll or posting a foreign document.

**Cause**: there is no rate for that pair on or before the date (the inverse pair also counts). Granary never
falls back to 1.0.

**Fix**: add the rate in **Finance → Exchange Rates** (or **Sync now** for today's rates), or type the exchange rate on the
document where the form has the field. The sync only stores today's rates, so add past rates by hand. If no
rates arrive at all, see [FX rates not syncing](#fx-rates-not-syncing-or-exchange_rates-is-empty).

---

## "Fiscal period ... is locked"

**Symptom**: `Fiscal period October 2026 is locked. Cannot post journal entries.` (or `Cannot create journal
entries.` for a manual entry).

**Cause**: the entry's date falls in a locked period. Automatic postings, reversals and manual entries all
check the lock.

**Fix**: date the document in an open period, or have finance unlock the period in **Finance → Periods**
(permission `finance:post_journal`). Lock it again after posting.

---

## Payroll: "A payroll run for MM/YYYY already exists" (409)

Only one non-cancelled payroll run per month is allowed. Open the existing run in **HR → Payroll**, or cancel it
first if it must be redone.

---

## The backend restarts in a loop after an upgrade

**Symptom**: `docker compose logs backend` shows `granary-migrate` failing, and the container restarts.

**Cause**: a migration aborted (it runs in one transaction and rolls back). A migration that adds a unique index
aborts when existing rows break it, and the error lists the offending values. The backend command is
`granary-migrate && granary-seed && exec granary-api`, so the API never starts.

**Fix**: read the error. For duplicates, run the matching query from
[Maintenance → Duplicate checks](MAINTENANCE.md#duplicate-checks), resolve them, then:

```bash
docker compose restart backend
docker compose exec -T backend granary-migrate --status | tail -3
```

---

## Personal mailbox not syncing

Each account folder is synced by one process at a time: a Redis lease `mail:sync:<account>:<folder>` with TTL
`mail.sync_lock_ttl_seconds` (default 600). A sync that finds the lease taken skips that pass; a lease left by a
crashed worker expires after the TTL.

```bash
docker compose logs worker | grep -E "mail_sync_not_configured|Synced"
docker compose exec redis redis-cli --scan --pattern 'mail:sync:*'
docker compose exec redis redis-cli TTL 'mail:sync:<account-id>:INBOX'
```

IMAP errors (wrong password, server unreachable) are stored on the account and shown in **Email**.

---

## The login rate limiter isn't limiting ("login rate limit skipped" or "unavailable" in the log)

The backend's sign-in limiter **fails open** on purpose: when Redis is unreachable, or `auth.login_max_attempts`
or `auth.login_window_seconds` is missing, it lets the sign-in through and logs an error rather than locking
everyone out.

```bash
docker compose logs backend | grep -E "login rate limit (skipped|unavailable)"
```

- `login rate limit skipped: Setting 'auth.login_max_attempts' is not set ...`: set both values in
  **Settings → System & security → Login security**.
- `login rate limit unavailable (Redis error) ...`: check `docker compose ps redis` and `REDIS_URL`.

Nginx's own limit on `/api/v1/auth/login` (5 per minute, burst 10) still applies meanwhile.
