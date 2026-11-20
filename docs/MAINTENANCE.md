# Maintenance

## Database backups

### Manual backup

```bash
docker compose exec -T db pg_dump -U granary granary \
  | gzip > backup_$(date +%Y%m%d_%H%M%S).sql.gz
```

### Restore

```bash
gunzip -c backup_20260101_120000.sql.gz \
  | docker compose exec -T db psql -U granary granary
```

### Automated daily backup (cron)

```bash
crontab -e
```

Add (replace `/path/to/granary` with your path):

```
0 2 * * * cd /path/to/granary && \
  docker compose exec -T db pg_dump -U granary granary \
  | gzip > /var/backups/granary/backup_$(date +\%Y\%m\%d).sql.gz && \
  find /var/backups/granary -name "*.sql.gz" -mtime +30 -delete
```

This runs at 02:00 every day and keeps 30 days of backups.

### Volumes worth backing up too

| Volume | Holds | Losing it means |
|---|---|---|
| `storage_data` | Uploaded files, documents, attachments, call recordings | The files are gone |
| `signing_keys` | The e-signature seal certificate (`platform.p12`) | New documents are sealed with a new certificate; already-signed PDFs keep their embedded signature |
| `stt_models` | The downloaded speech-to-text model | It downloads again on first use |

```bash
docker run --rm -v granary_signing_keys:/v -v "$PWD":/b alpine tar czf /b/signing_keys.tgz -C /v .
```

Volume names carry the Compose project name as a prefix; check them with `docker volume ls`.

---

## Updating the application

```bash
git pull
docker compose up -d --build
```

This rebuilds and restarts what changed. The backend applies new migrations when it starts (`granary-migrate`),
then runs the idempotent seed, which adds any new default settings without touching edited ones. If the server
is too small to compile Rust, build and load the images as in
[Deployment → Building the images elsewhere](DEPLOYMENT.md#building-the-images-elsewhere). Before an update that
adds unique indexes, run the [duplicate checks](#duplicate-checks): a migration that finds duplicates aborts and
the backend won't start.

Rebuilding part of the stack:

```bash
docker compose up -d --build frontend              # frontend only
docker compose up -d --build backend worker beat   # backend code: keep worker and beat on the same image
```

If `infra/nginx/granary.conf` changed, apply the change **by hand** to the live file. Don't copy the repository
file over it: certbot added the HTTPS directives to the live file, and copying would remove them.

```bash
sudo cp /etc/nginx/sites-available/granary /etc/nginx/sites-available/granary.bak-$(date +%Y%m%d-%H%M)
sudo nano /etc/nginx/sites-available/granary      # add or change only the blocks that changed
sudo nginx -t && sudo systemctl reload nginx        # reload only if the test passes
```

### Migration status

```bash
docker compose exec -T backend granary-migrate --status    # applied and pending migrations
```

Migrations run automatically on every backend start; there is no separate step.

---

## FX rate sync

Exchange rates are synced from ExchangeRate-API every 6 hours (job `sync_exchange_rates`). To check:

```bash
docker compose logs worker | grep -E "sync_exchange_rates|FX sync complete"

docker compose exec db psql -U granary granary \
  -c "SELECT from_currency, to_currency, rate, effective_date, source FROM exchange_rates ORDER BY effective_date DESC LIMIT 10;"
```

To sync now, use **Sync now** in **Settings → Money & tax → Currencies** or **Finance → Exchange Rates** (admin).

The sync uses `EXCHANGERATE_API_KEY` from `.env`, or the key saved in Settings when that is empty. With no key
it logs `No ExchangeRate API key configured` and stores nothing. A foreign-currency document whose date has no
rate on record is refused, so after a sync outage check the table and add missing rates by hand in
**Finance → Exchange Rates**.

---

## Email campaign queue

Campaign emails are sent by a loop inside the **backend** (not a scheduled job). Only the process holding the
Redis lease `lock:marketing.email_queue_worker` sends. It sends one email, waits a random
`marketing.email_min_delay_seconds` to `marketing.email_max_delay_seconds` (default 30 to 60 s), and stops for
the day at `marketing.email_daily_limit` (default 100, counted from midnight in the company time zone). Mail goes
out through a CRM inbox.

```bash
docker compose logs backend | grep -iE "Marketing email|queue worker"

# Who holds the sender lease, and for how long (seconds; -2 = nobody)
docker compose exec redis redis-cli GET lock:marketing.email_queue_worker
docker compose exec redis redis-cli TTL lock:marketing.email_queue_worker

# Queue depth
docker compose exec db psql -U granary granary \
  -c "SELECT status, count(*) FROM email_queue_items GROUP BY status;"
```

Redis needs its password; Compose sets `REDISCLI_AUTH` from `REDIS_PASSWORD` in the redis container, so
`redis-cli` inside it works as shown.

---

## Scheduled messages

Scheduled CRM emails and WhatsApp messages are sent by the job `send_scheduled_messages` every 5 minutes. Each
message is claimed (`pending` to `sending`) before it is sent, so it can't go out twice. A message stuck in
`sending` longer than `crm.scheduled.stuck_after_minutes` (default 30) is marked failed ("Interrupted while
sending; check the chat before resending") rather than resent; failures are retried up to
`crm.scheduled.max_attempts` (default 12). A WhatsApp message is sent only while its author still holds
`crm:send_whatsapp`.

```bash
docker compose logs worker | grep -i scheduled
```

---

## Background jobs

The schedule is in `rust-version/crates/worker/src/beat.rs`; the full list is in
[Architecture → Background jobs](ARCHITECTURE.md#background-jobs). `beat` only enqueues; the `worker` container
runs the jobs. Both run the backend image, so rebuild and restart them together.

A job whose settings are missing logs an error and skips its run, for example `invoice_reminders_not_configured`,
`scheduled_messages_not_configured`, `signature_reminders_not_configured` (worker log) or
`mail_sync_not_configured`. Check after each upgrade:

```bash
docker compose logs --since 24h worker | grep -E "not_configured|failed"
docker compose logs --since 24h beat | tail -20
```

---

## Settings to review periodically

| When | What | Where |
|---|---|---|
| When regulations change | Payroll contributions, wage caps and income tax brackets (`payroll.contributions`, `payroll.income_tax`), overtime rules | Settings → People & payroll |
| When a tax rate changes | Tax rows; `finance.default_sales_tax` | Finance → Taxes; Settings → Money & tax → Accounting |
| When a currency is activated | Its entry in `currency.decimals` | Settings → Money & tax → Accounting |
| When prices or headcount change | Approval policy amount limits | Settings → Approvals & signing |
| Weekly | FX rates are current | Finance → Exchange Rates |
| Monthly, at close | Lock the closed fiscal period | Finance → Periods |
| Each new year | Fiscal periods exist for the year (the seed creates only the install year's) | Finance → Periods |

---

## Duplicate checks

Each query must return no rows. The unique indexes prevent new duplicates; the queries help to confirm that a
restored backup is clean, or to find the rows a migration refused.

```bash
docker compose exec db psql -U granary granary
```

```sql
-- duplicate automatic journal entries
SELECT source, source_id, count(*) FROM journal_entries
WHERE is_deleted = false AND source IS NOT NULL
  AND source NOT IN ('manual', 'reversal', 'fx_auto', 'asset_depreciation')
GROUP BY source, source_id HAVING count(*) > 1;

-- two live payroll runs for one month
SELECT year, month, count(*) FROM payroll_runs WHERE status <> 'cancelled'
GROUP BY 1, 2 HAVING count(*) > 1;

-- the same vendor invoice number twice for one vendor (case-insensitive)
SELECT vendor_id, lower(vendor_invoice_number), count(*) FROM vendor_invoices
WHERE is_deleted = false AND vendor_invoice_number IS NOT NULL
GROUP BY 1, 2 HAVING count(*) > 1;

-- two pending approval requests for one document
SELECT entity_type, entity_id, count(*) FROM approval_requests WHERE status = 'pending'
GROUP BY 1, 2 HAVING count(*) > 1;

-- the same mail imported twice
SELECT account_id, folder, imap_uid, count(*) FROM email_messages WHERE imap_uid IS NOT NULL
GROUP BY 1, 2, 3 HAVING count(*) > 1;

-- two SLA policies for one team and priority
SELECT team_id, priority, count(*) FROM helpdesk_sla_policies
GROUP BY 1, 2 HAVING count(*) > 1;
```

How to resolve: reverse or delete the extra journal entries; cancel the extra payroll run; correct or delete the
duplicate vendor invoice; cancel the extra approval request; delete the duplicate `email_messages` rows (keep one
per UID); keep one SLA policy per team and priority. Take a backup first.

---

## Mail server: let website form emails through

Nary can create leads from website form notification emails (the lead source `web3forms`). Those notifications
are sent by a form service and often score high on spam filters, so they can land in Junk. Nary also reads them
from Junk (`nary.web3forms.poll_junk`, only messages from `nary.web3forms.sender_domains`), but allow the sender on
the mail server so they reach the Inbox:

- SpamAssassin: `whitelist_auth *@web3forms.com` in `local.cf` (authenticated by DKIM/SPF, so a forged sender
  isn't allowed), then restart spamd or amavis.
- Hosted mail: add `web3forms.com` to the spam filter's allow list.
- Check with a test submission: it should arrive in the Inbox, and the lead should appear in the CRM within a
  few minutes (the inbox poll runs every 2 minutes, Nary's event dispatch every minute).

When the mail server adds `Authentication-Results`, Nary requires a DKIM pass for a domain in
`nary.web3forms.dkim_domains`; without that header it accepts the presence of a matching DKIM signature.

---

## Speech to text

The `stt` service (faster-whisper, int8, on the CPU) transcribes voice notes for Nary. Audio never leaves the
server. Its CPU share is capped (`STT_CPUS`, default 1.5) and it handles one recording at a time; the model
(`STT_MODEL`, default `small`) downloads into the `stt_models` volume on first use.

```bash
docker compose exec stt python -c "import urllib.request,json;print(json.load(urllib.request.urlopen('http://localhost:3005/health')))"
# Download the model at build time instead of on first use:
docker compose build --build-arg STT_PRELOAD=small stt
```

---

## AI

Published SOPs are re-embedded every night (`reindex_kb_embeddings`) and can be re-embedded from
**Settings → Nary & AI** after a model change. This month's usage and estimated cost are shown under
**Settings → Nary & AI → Budget**; every call is logged in `ai_usage_log`:

```bash
docker compose exec db psql -U granary granary -c \
  "SELECT feature, provider, model, count(*), round(sum(est_cost)::numeric, 4) AS usd
   FROM ai_usage_log WHERE created_at >= date_trunc('month', now())
   GROUP BY 1,2,3 ORDER BY usd DESC;"
```

---

## View logs

```bash
docker compose logs --tail=100 -f          # all services
docker compose logs -f backend             # granary-api
docker compose logs -f frontend            # Next.js
docker compose logs -f worker              # background jobs
docker compose logs -f beat                # job schedule
docker compose logs -f pdf                 # PDF renderer
docker compose logs -f face                # face matching
docker compose logs -f research            # web research
sudo tail -f /var/log/nginx/error.log
sudo tail -f /var/log/nginx/access.log
```

The backend, worker and research service log at `info` by default; set `RUST_LOG` (for example
`RUST_LOG=debug`) in `.env` or the container's environment for more.

---

## Access the database

```bash
docker compose exec db psql -U granary granary
```

---

## Browsing the stored files

The storage (SeaweedFS) speaks S3 on `127.0.0.1:9000`. Any S3 client works with the keys from `.env`, for example
through an SSH tunnel:

```bash
ssh -L 9000:localhost:9000 user@your-server
aws --endpoint-url http://localhost:9000 s3 ls s3://granary-documents/   # MINIO_ACCESS_KEY / MINIO_SECRET_KEY
```

SeaweedFS's own admin shell also works: `docker compose exec storage weed shell`, then `s3.bucket.list`.

---

## Disk usage

```bash
docker system df -v       # Docker disk usage
docker image prune -f     # remove dangling images
```

---

## Missing buckets

Granary creates each bucket the first time it stores a file there, so a fresh or emptied storage needs no setup.
