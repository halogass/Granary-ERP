# Backups and Upgrades

<!-- nav:top -->
[Home](Home) › Administration › **Backups and Upgrades**

**On this page:** [What to back up](#what-to-back-up) · [Upgrade Granary](#upgrade-granary) · [Duplicate checks](#duplicate-checks) · [Routine checks](#routine-checks) · [Background jobs](#background-jobs) · [Logs and the database](#logs-and-the-database) · [Browsing the stored files](#browsing-the-stored-files) · [Disk usage](#disk-usage)
<!-- /nav:top -->

This page is for the person who runs the Granary server. It covers what to back up, how to restore, how to
update Granary to a newer version, and the routine checks that keep it healthy. All commands run in the Granary
folder on the server.

---

## What to back up

| What | Holds | If you lose it |
|---|---|---|
| The PostgreSQL database | All business data and settings | Everything is gone |
| Volume `storage_data` | Uploaded files, documents, attachments, call recordings | The files are gone |
| Volume `signing_keys` | The e-signature seal certificate (`platform.p12`) | New documents are sealed with a new certificate; already-signed PDFs keep their embedded signature |
| Your `.env` file | Secrets and addresses | Encrypted settings can't be read without the same `APP_SECRET_KEY` |
| Volume `stt_models` | The downloaded speech-to-text model | It downloads again on first use; no need to back up |

> Keep a copy of `.env` (at least `APP_SECRET_KEY`) somewhere safe and separate. Mail passwords and API keys in
> Settings are encrypted with it.

Volume names carry the Compose project name as a prefix (for example `granary_storage_data`). Check them with
`docker volume ls`.

### Database backup

```bash
docker compose exec -T db pg_dump -U granary granary \
  | gzip > backup_$(date +%Y%m%d_%H%M%S).sql.gz
```

### Automatic daily backup

Add a cron job (`crontab -e`), replacing `/path/to/granary`:

```
0 2 * * * cd /path/to/granary && \
  docker compose exec -T db pg_dump -U granary granary \
  | gzip > /var/backups/granary/backup_$(date +\%Y\%m\%d).sql.gz && \
  find /var/backups/granary -name "*.sql.gz" -mtime +30 -delete
```

This runs at 02:00 every day and keeps 30 days.

### Volumes

```bash
docker run --rm -v granary_signing_keys:/v -v "$PWD":/b alpine tar czf /b/signing_keys.tgz -C /v .
docker run --rm -v granary_storage_data:/v -v "$PWD":/b alpine tar czf /b/storage_data.tgz -C /v .
```

For a consistent copy of the files, stop the storage first (`docker compose stop storage`) and start it again
afterwards. You can also copy the buckets with any S3 client (see
[Browsing the stored files](#browsing-the-stored-files)).

Copy backups off the server.

### Restore the database

```bash
gunzip -c backup_20260101_120000.sql.gz \
  | docker compose exec -T db psql -U granary granary
```

After a restore, run the [duplicate checks](#duplicate-checks) to confirm the data is clean.

---

## Upgrade Granary

1. **Take a database backup** (above).
2. Get the new version: check out its tag, set `GRANARY_VERSION` in `.env` to it, and pull the images:

   ```bash
   git fetch --tags && git checkout v1.0.2   # the new release
   docker compose pull
   docker compose up -d
   docker compose logs -f backend            # watch until "listening"
   docker compose exec -T backend granary-migrate --status | tail -3   # newest migration applied
   ```

3. Check the worker for jobs that skip because a setting is missing:

   ```bash
   docker compose logs --since 24h worker | grep -E "not_configured|failed"
   ```

What happens on start: the backend applies new migrations (`granary-migrate`), then runs the seed, which adds new
default settings without touching the ones you edited. There is no separate migration step.

- **Keep `backend`, `worker` and `beat` on the same image.** They share one image, so recreate all three
  together. Otherwise the schedule may queue a job the worker doesn't know.
- **Built from source?** Run `docker compose build` after the checkout instead of `docker compose pull` (see
  [Installation](Installation#building-from-source)).
- **If a migration fails**, the backend exits and restarts, and nothing after it is applied. Read
  `docker compose logs backend`, fix the data it names (often duplicates, see below), then
  `docker compose restart backend`.

### Nginx changes

If `infra/nginx/granary.conf` changed in the new version, apply the change **by hand** to the live file. Don't copy
the repository file over it: Certbot added the HTTPS lines to the live file only.

```bash
sudo cp /etc/nginx/sites-available/granary /etc/nginx/sites-available/granary.bak-$(date +%Y%m%d-%H%M)
sudo nano /etc/nginx/sites-available/granary      # change only the blocks that changed
sudo nginx -t && sudo systemctl reload nginx
```

---

## Duplicate checks

Some upgrades add unique indexes. A migration that finds duplicates aborts, and the backend won't start until they
are resolved. Run these before such an upgrade, or after a restore. Each query must return no rows.

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

-- the same vendor invoice number twice for one vendor
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

To resolve (back up first): reverse or delete the extra journal entries; cancel the extra payroll run; correct or
delete the duplicate vendor invoice; cancel the extra approval request; keep one `email_messages` row per UID; keep
one SLA policy per team and priority.

---

## Routine checks

| When | What | Where |
|---|---|---|
| Daily | The backup ran and is copied off the server | Your backup folder |
| Weekly | Exchange rates are current | **Finance > Exchange Rates** |
| Monthly, at close | Lock the closed fiscal period | **Finance > Fiscal Periods** |
| Each new year | Fiscal periods exist for the new year (the seed creates only the install year) | **Finance > Fiscal Periods** |
| When a tax rate changes | Tax rows and the default sales tax | **Finance > Taxes**; **Settings > Money & tax > Accounting** |
| When regulations change | Payroll contributions, wage caps, income tax brackets, overtime | **Settings > People & payroll** |
| When a currency is activated | Its entry in `currency.decimals` | **Settings > Money & tax > Accounting** |
| When prices or headcount change | Approval policy amount limits | **Settings > Approvals & signing** |
| After each upgrade | Jobs skipping with "not configured" | `docker compose logs worker` |

---

## Background jobs

`beat` puts scheduled jobs on the queue; `worker` runs them. A few of them:

| Job | Every | Purpose |
|---|---|---|
| `poll_crm_inbox` | 2 min | New mail and read flags in the CRM inboxes |
| `send_scheduled_messages` | 5 min | Scheduled email and WhatsApp messages |
| `sync_personal_mailboxes` | 5 min | Personal mailboxes |
| `fire_due_reminders` | 5 min | CRM follow-up reminders |
| `remind_overdue_approvals` | 1 h | Approvers of overdue steps (once a day each) |
| `expire_quotations` | 1 h | Quotations past their validity |
| `sync_exchange_rates` | 6 h | FX rates from ExchangeRate-API |
| `send_invoice_reminders` | 1 day | Overdue invoices |
| `reindex_kb_embeddings` | 1 day | Re-embed published SOPs for Nary |

Email campaigns are not a scheduled job: they are sent by a loop inside the backend, one process at a time, paced
by `marketing.email_min_delay_seconds` to `marketing.email_max_delay_seconds` and capped by
`marketing.email_daily_limit`.

A job whose settings are missing logs an error (for example `invoice_reminders_not_configured`) and skips its run.

---

## Logs and the database

```bash
docker compose logs --tail=100 -f      # all services
docker compose logs -f backend         # API
docker compose logs -f worker          # background jobs
docker compose logs -f beat            # job schedule
docker compose logs -f frontend
sudo tail -f /var/log/nginx/error.log
docker compose exec db psql -U granary granary   # a database shell
```

Set `RUST_LOG=debug` in `.env` (then restart) for more detail from the backend, worker and research service.

AI usage this month is shown under **Settings > Nary & AI > Budget**.

---

## Browsing the stored files

The storage (SeaweedFS) speaks S3 on `127.0.0.1:9000` on the server. It has no web console. Use any S3 client with
`MINIO_ACCESS_KEY` and `MINIO_SECRET_KEY` from `.env`, through an SSH tunnel:

```bash
ssh -L 9000:localhost:9000 user@your-server
aws --endpoint-url http://localhost:9000 s3 ls s3://granary-documents/
```

Or use SeaweedFS's admin shell: `docker compose exec storage weed shell`, then `s3.bucket.list`.

Buckets need no setup: Granary creates each one the first time it stores a file there. The storage reads its
credentials when it starts, so after changing them in `.env` run `docker compose up -d storage`.

---

## Disk usage

```bash
docker system df -v
docker image prune -f      # remove dangling images
```

---

## Related pages

- [Installation](Installation)
- [Troubleshooting](Troubleshooting)
- [Configuration](Configuration)

<!-- nav:bottom -->
---

← Previous: [Administration](Administration) | Next: [Troubleshooting](Troubleshooting) →
<!-- /nav:bottom -->
