# Configuration

<!-- nav:top -->
[Home](Home) › Administration › **Configuration**
<!-- /nav:top -->

Granary has two kinds of configuration. **Environment variables** in `.env` hold secrets, service addresses and
infrastructure; changing one needs a container restart. **System settings** hold every business rule and number
(tax defaults, payroll contributions, approval limits, reminder schedules, limits, currency decimals); they live
in the database and you edit them in the web interface. This page explains how settings work, lists the settings
areas and describes the environment variables. It is for the Owner and system admins.

![Settings with the area list on the left and the search box](images/configuration-settings.png)

---

## How settings work

Open **Settings > General** in the sidebar. Settings are grouped in ten areas, each one long page of sections
with a save bar at the bottom:

1. Pick an area on the left (or type in the search box at the top: it finds any setting by its label or key).
2. Change the values. Changed fields are kept as a draft until you save.
3. Click **Save** in the save bar.

Who can change them: the Owner (superadmin) and system admins. Other staff can't change system settings.

### Business numbers are settings, never code

Rates, caps, thresholds, schedules, retry limits, default tax picks, approval limits and currency decimals are all
settings. None of them is a number hidden in the code.

- **Seeding.** On every backend start, the seed adds each setting that is missing, with its default. A value you
  have edited is never overwritten.
- **A missing or malformed value is a clear error.** The action fails with a message that names the setting and
  where to fix it, for example: `Setting 'projects.default_hourly_cost' is not set. Set it in Settings →
  Operations → Projects.` There is never a silent fallback number. The location named in an old message may be
  out of date: type the key into the Settings search box to find it.
- **Background jobs** whose settings are missing log the same message and skip their run. The next run picks up
  the fix without a restart.
- **One exception:** the sign-in rate limiter lets sign-ins through when its settings are missing (or Redis is
  down) and logs an error, so a mistake can't lock everyone out.
- **Business dates** follow the `timezone` setting (company time). Technical timestamps stay in UTC.

### Secrets in Settings

API keys and passwords entered in Settings (AI provider keys, mail passwords, the ExchangeRate-API key, the
GitLab token) are stored **encrypted** with `APP_SECRET_KEY`. They are never shown again after saving: a field
that holds one says it is set, and you leave it blank to keep it. When the API and the worker start, they check
these values too and alert the Owner about any that still look like a placeholder.

---

## Settings areas

| Area | Sections | Read more |
|---|---|---|
| **Company & branding** | Company identity, Document numbers, Regional (time zone, currency, date format), App logo, Favicon, Login page text, Letterhead, Knowledge & documents | [First Steps](First-Steps) |
| **Sales & CRM** | Funnel, CRM attributes, Deleted activities, Follow-up reminders, Quotations, Quotation notes templates, Pricing rules, Pricing & payment terms, Proposals, Cost breakdown & rate card, Partners, Scheduled messages | [CRM](CRM), [Sales](Sales) |
| **Money & tax** | Currencies, Accounting (ledger posting, decimals, default sales tax), Invoices (due dates, overdue reminders), Taxes (a link to Finance > Taxes) | [Finance](Finance), [Multi-Currency](Multi-Currency) |
| **People & payroll** | Attendance, Office locations, Kiosk tablets, Payroll, Overtime | [HR and Payroll](HR-and-Payroll), [Face Clock-In](Face-Clock-In) |
| **Nary & AI** | AI features, Providers & keys, Model per job, Budget, Chat persona, AI chat, Web research, Company enrichment, Lead-scoring rubric, AI knowledge, and the sections on how Nary works (day, autonomy, replies, intake, scoring, follow-up cadence, email, meetings, work queue) | [Nary AI Assistant](Nary-AI-Assistant) |
| **Email & WhatsApp** | Email signature, Test recipient, System email, CRM inboxes, Mail sync, Email campaigns, WhatsApp link | [CRM Inboxes](CRM-Inboxes), [Marketing](Marketing) |
| **Approvals & signing** | Approval policies, E-signing | [Approvals and Signing](Approvals-and-Signing) |
| **Operations** | Projects, Procurement, Manufacturing, Fleet & maintenance | [Projects](Projects), [Procurement](Procurement), [Operations](Operations) |
| **Integrations & access** | MCP access, Users & roles, GitLab | [MCP for AI Clients](MCP-for-AI-Clients), [Administration](Administration), [R&D](RnD) |
| **System & security** | Login security, Public pages, Other settings | [Administration](Administration) |

**Other settings** (under System & security) lists any setting the server has that no other section shows.

### Settings you will meet often

| Key | Default | What it does |
|---|---|---|
| `timezone` | `UTC` | Company time zone for business dates. Must be a valid zone name such as `Asia/Tokyo` |
| `currency.functional` | `USD` | Base currency of the ledger. Can't change once journal entries exist |
| `currency.decimals` | JPY 0, others 2 | Minor-unit digits per currency for all rounding |
| `finance.default_sales_tax` | `{}` | Tax code preselected per currency when a quotation becomes an invoice |
| `finance.gl_autopost_from` | blank (off) | Date from which documents post to the ledger automatically |
| `finance.invoice_reminder_days` | `[1, 7, 14, 30]` | Overdue-invoice reminders, days after the due date |
| `payroll.contributions`, `payroll.income_tax` | `[]`, `{"method": "none"}` | Statutory contributions and income tax for payroll |
| `projects.default_hourly_cost` | blank | Hourly cost for people with no employee record; logging their time fails until it is set |
| `auth.login_max_attempts`, `auth.login_window_seconds` | `5`, `900` | Failed sign-ins allowed per address per window |
| `marketing.email_daily_limit` | `100` | Campaign emails per day |
| `ai.budget.monthly_usd` | `100` | Monthly AI budget (0 = no cap) |
| `mcp.apply.require_totp` | on | AI clients need the authenticator code to apply sensitive actions |

The repository's `docs/CONFIGURATION.md` has the full reference with every default.

---

## Environment variables

The backend, worker and beat read `.env` (and real environment variables, which win). The required ones have no
default: the backend does not start without them. Changing a variable needs a restart of the containers that
read it (`docker compose up -d backend worker beat`, and `--build frontend` for the URLs baked into the frontend).

### Keys and secrets

| Variable | Notes |
|---|---|
| `APP_SECRET_KEY` | Required. The master key; also encrypts secrets in Settings. Set it once, before real data |
| `JWT_SECRET_KEY` | Required, at least 32 characters. Signs sign-in tokens. Changing it signs everyone out |
| `NEXTAUTH_SECRET` | Required by Compose. The frontend's session key, its own value |
| `SIGNAL_TICKET_KEY` | Required by Compose. Signs video-call tickets |
| `INTERNAL_AUTH_TOKEN` | Lets the frontend pass the visitor's real address to the sign-in rate limiter |
| `PDF_SERVICE_TOKEN`, `FACE_SERVICE_TOKEN`, `RESEARCH_SERVICE_TOKEN`, `STT_SERVICE_TOKEN`, `SEARXNG_SECRET` | Random tokens between containers |
| `JWT_ACCESS_TOKEN_EXPIRE_MINUTES`, `JWT_REFRESH_TOKEN_EXPIRE_DAYS`, `JWT_REFRESH_GRACE_SECONDS` | `60`, `30`, `60`. Session lengths |

**Placeholder secrets.** The API and the worker refuse to start while `JWT_SECRET_KEY`, `APP_SECRET_KEY` or
`NEXTAUTH_SECRET` still begins with the `.env.example` placeholder (`change-this`, also `change_this` or
`changethis`). Other weak values (shorter than 32 characters for generated keys, or starting with `test`,
`secret`, `password`, `example`, `changeme` and similar) are logged and the Owner is alerted once per start,
without stopping it.

### Database and Redis

| Variable | Notes |
|---|---|
| `POSTGRES_PASSWORD`, `DATABASE_URL` | Required. The URL must carry the same password |
| `POSTGRES_HOST`, `POSTGRES_PORT`, `POSTGRES_DB`, `POSTGRES_USER` | `db`, `5432`, `granary`, `granary` |
| `REDIS_PASSWORD` | Required by Compose |
| `REDIS_URL`, `CELERY_BROKER_URL`, `CELERY_RESULT_BACKEND` | Redis databases 0, 1 and 2, each carrying the password. The `CELERY_*` names are historical: the job queue is Granary's own |

### S3 storage settings (the names date from the earlier MinIO setup)

| Variable | Default | Notes |
|---|---|---|
| `MINIO_ENDPOINT` | `storage:9000` | The S3 endpoint inside Docker (the `storage` service, SeaweedFS) |
| `MINIO_ACCESS_KEY`, `MINIO_SECRET_KEY` | `granary`, required | The storage credentials. The `storage` service reads them when it starts |
| `MINIO_BUCKET_DOCUMENTS`, `MINIO_BUCKET_ASSETS`, `MINIO_BUCKET_PUBLIC` | `granary-documents`, `granary-assets`, `granary-public` | Only the public bucket can be read without credentials. Granary creates each bucket on first use |
| `MINIO_USE_SSL` | `false` | |
| `MINIO_PUBLIC_URL` | | Where browsers reach the storage: in production `https://<your domain>/minio` |

Any S3-compatible service works in place of the shipped SeaweedFS.

### URLs

| Variable | Notes |
|---|---|
| `APP_URL` | The public address; used in email links and as the frontend's sign-in URL |
| `API_URL`, `INTERNAL_API_URL` | Keep `http://backend:8000`. Never the public domain |
| `SIGNAL_URL` | Public WebSocket URL for video calls, `wss://<your domain>/ws/signal`. Baked into the frontend |
| `CORS_ORIGINS` | A JSON list, for example `["https://granary.example.com"]` |

### Sidecars, calls and signing

| Variable | Notes |
|---|---|
| `PDF_SERVICE_URL`, `PDF_ASSET_HOSTS` | The PDF renderer; extra hosts it may load images from |
| `FACE_SERVICE_URL`, `FACE_MAX_CONCURRENT` | Face matching; analyses run at once |
| `RESEARCH_SERVICE_URL` | Web page reader for research |
| `STT_SERVICE_URL`, `STT_MODEL`, `STT_CPUS`, `STT_THREADS` | Speech to text: `small`, `1.5`, `1` |
| `SIGNAL_SERVICE_URL` | Backend to signalling service (internal) |
| `TURN_SECRET`, `TURN_EXTERNAL_IP`, `TURN_URIS` | The TURN server (see [Installation](Installation#turn-server-for-video-calls-coturn)) |
| `SIGN_CERT_P12_PATH`, `SIGN_CERT_P12_PASSWORD`, `SIGN_TSA_URL` | The e-signature seal certificate. Without one, a self-signed certificate is made on first use. For trusted signing, put a CA-issued `.p12` there |
| `SIGN_SEAL_NAME`, `SIGN_REASON`, `SIGN_LOCATION` | Text inside the seal |
| `SIGN_QES_ENABLED`, `SIGN_QTSP_URL`, `SIGN_QTSP_API_KEY` | Reserved; no remote signing provider is connected |
| `WHATSAPP_SERVICE_URL`, `WHATSAPP_SERVICE_TOKEN`, `WHATSAPP_BUSINESS_NUMBER` | A WhatsApp bridge you provide (not shipped) |
| `EXCHANGERATE_API_KEY` | Optional; used by the automatic FX sync before the key saved in Settings |

### Not in `.env`

Mail servers, AI providers and keys, company identity, taxes, payroll rules and the time zone are configured in
Settings, not in `.env`.

---

## Related pages

- [First Steps](First-Steps)
- [Administration](Administration)
- [Installation](Installation)
- [Troubleshooting](Troubleshooting#setting-x-is-not-set-400)

<!-- nav:bottom -->
---

← Previous: [Collaboration](Collaboration) | Next: [Administration](Administration) →
<!-- /nav:bottom -->
