# Configuration

After the first sign-in as the superadmin, work through these steps before inviting the team. Settings are under
**Settings** in the sidebar, grouped in areas: Company & branding, Sales & CRM, Money & tax, People & payroll,
Nary & AI, Email & WhatsApp, Approvals & signing, Operations, Integrations & access, and System & security. The
search box at the top of Settings finds any setting by name. Only the superadmin and system admins can change
system settings.

There are two kinds of configuration:

- **Environment variables** (`.env`, read by `rust-version/crates/core/src/config.rs`): secrets, service URLs and
  infrastructure. Changing one needs a container restart. See [section 19](#19-environment-variables).
- **System settings** (the `system_settings` table, edited in the UI): every business number and rule, such as
  tax defaults, payroll contributions, approval limits, reminder schedules, limits and currency decimals. None
  of them is hard-coded. See [section 16](#16-business-rules-are-settings) and the
  [reference in section 17](#17-system-settings-reference).

---

## 1. Company & branding

- **Company identity**: legal name, trade name, tax ID, registration number, address, signatory name and title.
  They appear on quotations, invoices and letters.
- **Document numbers**: quotation, invoice and proforma prefixes, default payment terms.
- **Regional**: time zone (`UTC` until you set yours, for example `Asia/Tokyo`), default currency (`USD`), date
  format. The `timezone` setting is the company time zone for business dates (see section 16).
- **App logo**: text (logo text and subtitle) or an uploaded SVG/PNG image.
- **Favicon**: a 32×32 or 64×64 PNG, SVG or ICO.
- **Letterhead**: the header image on printed documents.

---

## 2. Finance → Taxes

Sales and purchase taxes are configured in **Finance → Taxes** only.

A fresh install has no taxes. Add the ones you charge and withhold, for example **VAT10** (VAT 10%, type
`sales`) and **WHT2** (withholding tax on services 2%, type `withholding`).

- Invoice lines reference a tax from this table; the line keeps a copy of the tax code and rate, so a later rate
  change never rewrites an issued invoice.
- When a quotation becomes an invoice, the tax pre-selected for every line comes from the setting
  `finance.default_sales_tax` (per currency, for example `{"JPY": "VAT10"}`; default `{}`, none pre-selected). It
  can be changed per line on the draft invoice.
- New vendors that withhold tax get the rate in `procurement.default_withholding_rate` (percent, default 0).

Payroll income tax does not come from this table. It is computed from the `payroll.income_tax` setting
(section 5).

---

## 3. Email & WhatsApp

| Account | Purpose | Example |
|---|---|---|
| System email (SMTP) | Notifications, password resets, approval and signing emails | `erp@example.com` |
| CRM inboxes (SMTP + IMAP) | Shared inboxes under CRM, sending approved documents, email campaigns | `hello@example.com`, `sales@example.com` |

Set a **Test recipient**, then use the **Send test** buttons to check each account. Passwords are stored
encrypted in the database, not in `.env`. A new CRM inbox is switched on once its servers are set and tested,
and then imports its recent mail quietly; one inbox is the default.

The CRM inboxes mirror the mail server: they import the Sent folder, sync read status both ways, and save mail
sent from the ERP to the server's Sent folder. Opening a CRM thread marks it read for everyone (the inbox is
shared).

**Email signature**: set the company signature layout here; each user fills in their own fields from their
profile. The same signature is used by the CRM inboxes, personal mail and email campaigns.

Each user can also connect a **personal mailbox** under **Email** (per-user IMAP/SMTP). It syncs Inbox and Sent
every 5 minutes; each folder is synced by one process at a time (a Redis lease, TTL `mail.sync_lock_ttl_seconds`).

Email campaigns are sent through a CRM inbox, paced and capped by the `marketing.*` settings (section 17).

---

## 4. Approvals & signing

Approval policies per document type (quotation, proposal, invoice, purchase order, BOM, contractor request, work
order, vendor invoice, expense, marketing content, hiring request, offer letter, SOP, payroll run) are seeded on
the first start and edited in **Settings → Approvals & signing → Approval policies**. The seed never overwrites
your edits.

- **Set the amount limits for your currency.** Limits are amounts in the functional currency. The shipped values
  are sized for a currency like JPY: a purchase order above 5,000,000 goes to the Owner, a vendor invoice above
  25,000,000 goes to the Owner, an expense above 2,000,000 (or one line above 1,500,000) goes to the Owner. Set
  each limit, and the escalation thresholds on quotations and invoices, to sensible values for your base
  currency before the first document is submitted.
- When a guarded amount is unknown (a purchase order with no exchange rate on record, for example), every
  amount-guarded step applies.
- Quotations are approved automatically only when every line is a catalog item at list price with no discount.
- The Owner approves every invoice and credit note.
- A rejected document returns to draft and the chain restarts.
- Approved documents carry the approver's stamp and a verification QR, and can be sent to the customer. Emailed
  documents appear as a conversation under CRM.
- Invoices have an optional **billing email**, used before the CRM contact's address; it is the way to send an
  invoice to a customer who isn't in the CRM.
- Budgets are not in the approval engine: they are approved with **Approve** on the budget (permission
  `finance:manage_budget`).

**E-signing**: the largest signature image accepted and the signer reminder schedule (`sign.*`).

---

## 5. People & payroll

Face clock-in runs on the self-hosted `face` service: no outside face API and no key.

| Setting | Description |
|---|---|
| Face match strictness | Largest match distance, default `0.50` |
| GPS accuracy limit | Largest GPS error (metres) accepted for a clock-in, default 100 |
| Work start time | For late-arrival detection, default `09:00` |
| Office locations | GPS centre and radius (metres) per office; employees must be inside to clock in |
| Kiosk tablets | Registered clock-in tablets; revoke one here to sign it out |

### Payroll

- **Contributions** (`payroll.contributions`): the social security, health and pension contributions your
  jurisdiction requires, each with an employee rate, an employer rate, an optional monthly wage cap, and whether
  the employer share is taxable income or the employee share is deductible. An empty list means no
  contributions.
- **Income tax** (`payroll.income_tax`): `{"method": "none"}`, or progressive annual brackets with a yearly
  allowance per tax status, an optional work-expense deduction and rounding. Tax is withheld on the annualised
  year-to-date income, so every month corrects the months before it.
- **Overtime**: hours divisor, multipliers and rest days.

Payroll refuses to run while a setting it needs is empty or malformed, and the error names it. Check your local
rates before the first payroll run.

Recurring per-employee allowances (fixed and taxable flags) are set in **HR → Payroll**. Fixed allowances count
toward the contribution wage and the overtime hourly wage.

---

## 6. Onboard employees

### Create user accounts

1. **Settings → Users** → add a user for each team member.
2. Set the portal type:
   - `internal`: staff who use the ERP;
   - `partner`: partner contacts (the `/partner` portal only);
   - `vendor`: vendor and contractor contacts (the `/vendor` portal only).
3. Link each staff user to their **employee record**. The link connects the sign-in to attendance, leave,
   payroll and time logs.

Partners and vendors can also register themselves; approve them in **Admin → Portal users**.

### Enrol faces for clock-in

1. **HR → Employees** → **Face ID** on the employee.
2. Upload clear face photos (good light, face centred).
3. **Enrol**.

Only face descriptors (lists of numbers) are stored, never the photos.

---

## 7. Clock-in page

| Mode | URL | How it works |
|---|---|---|
| Personal (own phone or laptop) | `https://your-domain/clock` | Needs sign-in; a short liveness check, then a match against the signed-in employee only |
| Kiosk (shared tablet at the office) | `https://your-domain/clock?kiosk=1` | The tablet is registered first: open the URL on it and sign in as HR once. The tablet then identifies employees itself, and the server re-checks every match |

Location permission is required in both modes.

---

## 8. WhatsApp

Granary does not include a WhatsApp Web bridge. The WhatsApp inbox (CRM → WhatsApp), document sending over
WhatsApp, and Nary's WhatsApp channel call an HTTP service at `WHATSAPP_SERVICE_URL` with the bearer token
`WHATSAPP_SERVICE_TOKEN`, and receive its events on the backend's WhatsApp webhook. If you run such a service,
pair it from **Settings → Email & WhatsApp → WhatsApp link**. Without one, these features show as disconnected
and everything else works.

---

## 9. Nary & AI

Everything AI is configured under **Settings → Nary & AI** (superadmin and system admins). Keys are stored
encrypted.

| Section | What it holds |
|---|---|
| AI features | The master switch (off stops every AI feature, Nary included), no em dashes in AI output (default on), and switches for Nary chat, drafting quotations and invoices, lead scoring, reply suggestions and document intelligence |
| Providers & keys | API keys for Anthropic, OpenAI (or an OpenAI-compatible endpoint), Gemini, DeepSeek; Ollama needs no key |
| Model per job | `ai.routing`: provider, model and effort for each job (assistant, enrichment, research, embeddings, web search, each Nary task). The shipped routing uses Anthropic models for most jobs and Gemini for embeddings and web search: change it to the providers you have keys for |
| Budget | `ai.budget.monthly_usd` (default 100, 0 = no cap), the warning and stop percentages, and the jobs that keep running when the budget is spent; shows this month's usage |
| Chat persona, AI chat | How the assistant writes, what it knows about the ERP and the catalog |
| Web research | SearXNG (self-hosted, free) or another search backend; needed for CRM enrichment and deep research |
| Company enrichment | What enrichment may fill in by itself (`crm.enrich.*`) |
| AI knowledge | Markdown or PDF documents only the AI reads (pricing rules, product facts, playbooks), kept apart from the employee knowledge base and used by every AI feature and MCP connection; re-index after changing the embedding model |
| Nary's day, autonomy, replies, intake, scoring, follow-up cadence, email, meetings, work queue | How Nary works with customers and with the Owner, and what it may do without asking |

Test the connection after saving a key.

**Pausing Nary**: the `nary.paused` setting (default off) stops Nary's runs and actions; briefs still go out
and the work queue collects what waits.

**MCP: the authenticator code on `nary_apply`** (`mcp.apply.require_totp`, Integrations & access → MCP
access, default on). On a full-power connection, an AI client prepares a customer message, an invoice, a
meeting, a discount, a tax or a money change, a delete or a merge, and then calls `nary_apply` to apply it; the
client asks the person to Allow that call. With this setting on and two-factor sign-in on for the person,
`nary_apply` also needs the current code from their authenticator app, typed by the person in the chat. Turn it
off only if you accept the risk: then the Allow click alone applies the action, and an AI that has read a planted
instruction (in an email, a web page or a document) only needs one careless click to send, bill, delete or merge
in your name. People without two-factor sign-in are not asked for a code either way, so turn two-factor sign-in
on for everyone who connects an AI client at full power.

A connection that can act (act or full power) also needs the account password (and the authenticator code) on
the consent page, and ends `mcp.pat.max_days_power` days (default 90) after it was allowed, however often the
agent refreshes it.

---

## 10. CRM settings

- **Sales & CRM → Funnel**: stages, stage probabilities, loss and disqualify reasons, lead sources, priorities
  and activity types.
- **Sales & CRM → CRM attributes**: custom fields on organizations, contacts, leads and deals (including fields
  the AI fills).
- **Sales & CRM → Follow-up reminders**: default reminder delay, no-reply and stale-record thresholds, delivery
  window, the Owner's weekly digest day and time.
- **Integrations & access → MCP access**: personal access tokens that let your AI client (MCP) work in the ERP
  through `/api/v1/mcp` with that user's permissions, at the level you choose (read, propose or act).

The product catalog and product lines can be edited only by the Owner (superadmin) or the `executive` role.

---

## 11. Knowledge base

The knowledge base starts empty. Write articles and SOPs under **Knowledge**, or import PDFs. Published SOPs are
embedded for the assistant: on publish, and again every night (`reindex_kb_embeddings`). After changing the
embedding model in Settings → Nary & AI, re-index from there.

---

## 12. GitLab (optional)

**Settings → Integrations & access → GitLab**: the GitLab server, an access token, the repositories to link to
R&D projects and the webhook secret. The R&D backlog and GitLab issues then sync both ways every minute.

---

## 13. Currencies

In **Settings → Money & tax → Currencies** (admin only):

| Field | Default | Notes |
|---|---|---|
| Functional currency | USD | Ledger and KPI amounts are in this currency. It **can't be changed** once journal entries are posted (409). |
| Active currencies | USD, EUR | The currencies offered in forms. Supported: USD, EUR, GBP, JPY, SGD, AUD, CNY. |
| ExchangeRate-API key | none | A free key from [exchangerate-api.com](https://www.exchangerate-api.com). Stored encrypted. |

Money is rounded to each currency's minor unit from `currency.decimals` (Money & tax → Accounting; JPY 0, the
others 2). A currency missing from it gives a 400 naming the setting.

After saving the key, **Test connection**, then **Sync now** (also in **Finance → Exchange Rates**). Rates sync
automatically every 6 hours, with `EXCHANGERATE_API_KEY` from `.env` when it is set, otherwise with the saved key.

A foreign-currency document needs a rate on record for its date, or an exchange rate typed on the document.
There is never a silent 1.0: without a rate the action fails with "No USD/JPY exchange rate is on record for
...". Rates can be added by hand in **Finance → Exchange Rates**. With only the functional currency in use, no key is
needed.

---

## 14. Video calls: TURN server

Video calls are WebRTC peer to peer, with coturn for NAT traversal. Set in `.env`:

```dotenv
TURN_SECRET=<strong-random>          # coturn's --static-auth-secret, and the backend's TURN credentials
TURN_EXTERNAL_IP=<server-public-ip>
TURN_URIS=["turn:granary.example.com:3478"]
```

**TURN over TLS (optional).** coturn listens for TLS on 5349, but answers there only once it has a certificate.
To offer `turns:` (useful where networks allow only TLS):

1. Mount the certificate folder into the `coturn` service in `docker-compose.yml`, for example
   `- /etc/letsencrypt:/etc/letsencrypt:ro` under `volumes:`.
2. Uncomment the `cert=` and `pkey=` lines in `infra/coturn/turnserver.conf` and point them at the TURN host
   name's full chain and private key.
3. Add the URI: `TURN_URIS=["turn:granary.example.com:3478","turns:granary.example.com:5349"]`, then restart
   coturn and the API.

Without a certificate, keep `TURN_URIS` on `turn:` only: a `turns:` URI with no certificate behind it fails.

Open the firewall:

```bash
sudo ufw allow 3478/tcp 3478/udp 5349/tcp 5349/udp
sudo ufw allow 49152:65535/udp   # TURN relay range
```

---

## 15. E-signature certificate (optional)

Signed documents are PAdES-sealed with the certificate at `SIGN_CERT_P12_PATH` (in the `signing_keys` volume).
Without one, a self-signed certificate is generated on first use. For trusted signing, put a CA-issued `.p12`
there, set `SIGN_CERT_P12_PASSWORD`, and optionally `SIGN_TSA_URL` for RFC 3161 timestamps.

---

## 16. Business rules are settings

Rates, caps, thresholds, schedules, retry limits, default tax picks, approval limits and currency decimals are
`system_settings` rows, never numbers in the code.

- **Seeding:** the baseline migration and `granary-seed` (which runs on every backend start) insert every
  setting that is missing, with its default. An existing value is never overwritten.
- **Editing:** in Settings, in the area listed in section 17. A setting can also be changed through
  `PATCH /api/v1/system/settings/{key}` (superadmin or system admin). Settings that no area shows are under
  **System & security → Other settings**.
- **Missing or malformed value:** the action fails with **400** and a message naming the setting and where to
  fix it, for example `Setting 'projects.default_hourly_cost' is not set. Set it in Settings → Operations → Projects.`
  There is no fallback number. Background jobs log the same message and skip their run (see
  [Troubleshooting](TROUBLESHOOTING.md#setting-x-is-not-set-400)).
- **Exception:** the login rate limiter lets sign-ins through when its settings are missing (or Redis is down)
  and logs an error, so a configuration mistake can't lock everyone out.
- **Business dates** (document numbers and default dates, overdue checks, FX lookups, payroll, CRM close dates,
  the campaign daily cap) follow the `timezone` setting. There is no time-zone environment variable. Technical
  timestamps stay in UTC.

---

## 17. System settings reference

Defaults are the values seeded on a fresh install. Error messages may name an older location ("Settings →
General → ..."); use the Settings search box to find the key.

### Company, documents and regional (Company & branding)

| Key | Default | Notes |
|---|---|---|
| `company_name`, `company_trade_name`, `company_email`, `company_phone`, `company_website`, `company_address`, `company_rd_office`, `company_tax_id`, `company_registration_no`, `company_signatory`, `company_signatory_title` | `Your Company Ltd`, `Your Company`, `hello@example.com`, the rest blank | Company identity |
| `quotation_prefix`, `invoice_prefix`, `proforma_prefix` | `QUO`, `INV`, `PRO` | Document numbers |
| `payment_terms_default` | `14` | Days |
| `timezone` | `UTC` | Company time zone for business dates |
| `default_currency`, `date_format` | `USD`, `DD/MM/YYYY` | Regional |
| `invoice_due_days`, `quotation_validity_days`, `max_discount_pct_without_approval` | `14`, `30`, `15` | |

### Money & tax

| Key | Default | Notes |
|---|---|---|
| `currency.functional`, `currency.active`, `currency.api_key` | `USD`; `["USD", "EUR"]`; none | Currencies card (section 13) |
| `currency.decimals` | `{"USD": 2, "EUR": 2, "GBP": 2, "SGD": 2, "JPY": 0, "CNY": 2, "AUD": 2}` | Minor-unit digits per currency for all money rounding |
| `finance.gl_autopost_from` | blank (off) | Date (YYYY-MM-DD) from which documents post to the ledger automatically (section 18) |
| `finance.default_sales_tax` | `{}` | Tax code pre-selected per currency when a quotation becomes an invoice |
| `finance.invoice_reminder_days` | `[1, 7, 14, 30]` | Overdue-invoice reminders, days after the due date |
| `finance.invoice_reminder_repeat_days` | `30` | After the last listed day, remind every N days (0 = stop) |

### People & payroll

Rates are percent of the wage; amounts are in the functional currency.

| Key | Default | Notes |
|---|---|---|
| `payroll.contributions` | `[]` | `[{key, label, employee_rate, employer_rate, base_cap, employer_taxable, employee_deductible}, ...]`: one entry per statutory contribution |
| `payroll.income_tax` | `{"method": "none"}` | Or `{"method": "progressive", "brackets": [[annual upper bound or null, rate %], ...], "allowances": {status: yearly amount}, "default_status", "expense_deduction_rate", "expense_deduction_monthly_cap", "taxable_rounding"}` |
| `payroll.overtime_hours_divisor` | `173` | Hourly wage = monthly fixed wage ÷ this |
| `payroll.overtime_multipliers` | `{"workday": [[1, 1.5], [null, 2]], "rest_day": [[8, 2], [9, 3], [null, 4]]}` | `[cumulative hour upper bound, multiple of the hourly wage]` |
| `payroll.rest_weekdays` | `[5, 6]` | Rest days for overtime (Monday = 0). Days marked holiday in attendance also count |
| `hr.face_match_distance`, `hr.geo_max_accuracy_m`, `hr.work_start_time` | `0.50`, `100`, `09:00` | Attendance |

### Other areas

| Area | Key | Default | Notes |
|---|---|---|---|
| Sales & CRM | `crm.scheduled.max_attempts` | `12` | Scheduled messages: attempts before failing |
| Sales & CRM | `crm.scheduled.stuck_after_minutes` | `30` | A send stuck in "sending" this long is failed, not resent |
| Operations | `projects.monthly_hours` | `173` | Hourly cost of time logs = base salary ÷ this (must not be 0) |
| Operations | `projects.default_hourly_cost` | blank | Hourly cost for users with no employee record; logging time for such a user fails until it is set |
| Operations | `procurement.receipt_over_tolerance_pct` | `5` | A goods receipt may exceed the ordered quantity by this % |
| Operations | `procurement.default_withholding_rate` | `0` | Withholding tax % for new vendors that withhold it |
| Approvals & signing | `sign.signature_max_bytes` | `500000` | Largest signature image accepted |
| Approvals & signing | `sign.reminder_window_hours` | `48` | Remind signers when the deadline is this close |
| Approvals & signing | `sign.reminder_interval_hours` | `24` | Hours between reminders to the same signer |
| System & security | `auth.login_max_attempts` | `5` | Failed sign-ins allowed per address per window (then 429) |
| System & security | `auth.login_window_seconds` | `900` | Rate-limit window |
| Email & WhatsApp | `marketing.email_daily_limit` | `100` | Campaign emails per day (company time zone) |
| Email & WhatsApp | `marketing.email_min_delay_seconds` / `marketing.email_max_delay_seconds` | `30` / `60` | Random pause between sends; max must not be below min |
| Email & WhatsApp | `marketing.queue_lock_ttl_seconds` | `180` | Sender lease TTL; must be longer than the max delay |
| Email & WhatsApp | `mail.sync_lock_ttl_seconds` | `600` | Lease per personal-mailbox folder sync |

### CRM and AI

- `crm.followup.*` (Sales & CRM → Follow-up reminders): default delay (`3` days), no-reply (`3`), idle lead
  (`7`), deal stage idle (`14`), quotation follow-up (`5`) days, `max_per_rule_pass` (`25`), delivery window
  `08:00` to `20:00`, the Owner's weekly digest on weekday `1` (Monday) at `08:00`, and an on/off switch per rule.
- `crm.enrich.*` (Nary & AI → Company enrichment): research on create (`true`), the fields it may fill,
  minimum confidence (`70`), refresh after (`30` days), daily limit (`50`).
- `ai.*` and `nary.*` keys are managed under Nary & AI (section 9).

---

## 18. GL account mapping and automatic posting

Operational documents post their own journal entries once **automatic posting** is on. Each posting line names
an account **role** (for example `ar`, `sales_tax_output`, `salary_expense`, `expense.travel`), and finance maps
each role to an account.

**Account mapping:** **Finance → Chart of Accounts → Account mapping** (permission `finance:manage_accounts`).
The seed maps every role to the default chart's accounts:

| Role | Default | Role | Default |
|---|---|---|---|
| `ar` | 1300 | `income_tax_payable` | 2200 |
| `customer_advances` | 2050 | `contributions_payable` | 2450 |
| `revenue` | 4100 | `employee_payable` | 2410 |
| `sales_tax_output` | 2100 | `salary_expense` | 5200 |
| `sales_tax_input` | 1450 | `employer_contributions_expense` | 5250 |
| `ap` | 2000 | `expense_default` | 5950 |
| `vendor_invoice_expense` | 5000 | `bank_default` | 1100 |
| `withholding_tax_payable` | 2300 | `fx_gain_loss` | 4500 |
| `retained_earnings` | 3100 | `asset_disposal_gain_loss` | 4600 |
| `current_earnings` | 3200 | `expense.<category>` | travel and meals 5600; office supplies, hardware and utilities 5400; software 5700; marketing 5500; professional services 5300; other 5950 |

An `expense.<category>` role with no mapping falls back to `expense_default`. An unmapped role, or a mapped
account that is inactive, makes the action fail with 400 ("No account is mapped to the 'x' role ..."); nothing is
half-done.

**Bank accounts:** a payment, vendor payment, payroll payout, reimbursement or refund that names a bank account
posts to that bank's **GL account**. Set it on the bank account in **Finance → Bank**. Without a
chosen bank, the `bank_default` role is used.

**Cut-over:** `finance.gl_autopost_from` (Money & tax → Accounting). Blank = off. Documents dated on or after it
post automatically; earlier ones are covered by the one-time `POST /api/v1/finance/accounting/migrate-opening`,
which takes only documents before the cut-over.

**When documents post:** a commercial invoice when sent; a payment when recorded (reversed when deleted); a
credit note when issued; a refund when paid; a vendor invoice on approval and on payment; an expense on approval
and when reimbursed; payroll when paid; asset depreciation and disposal when posted. Proformas never post (a
payment on a proforma is a customer advance). Each event posts once.

**Fiscal periods:** a posting into a locked fiscal period fails with "Fiscal period ... is locked". Lock, unlock
and create periods in **Finance → Periods** (permission `finance:post_journal`). The seed creates the twelve
periods of the install year only.

Before turning automatic posting on, follow
[Deployment → Before enabling GL auto-posting](DEPLOYMENT.md#before-enabling-gl-auto-posting).

---

## 19. Environment variables

Read from `.env` by the backend, the worker and beat (`env_file: .env` in `docker-compose.yml`), and by
`rust-version/crates/core/src/config.rs`. Real environment variables win over `.env`. Variables marked required
have no default: the backend does not start without them.

| Variable | Default | Notes |
|---|---|---|
| `APP_SECRET_KEY` | required | The backend's master key. The backend refuses to start while it starts with the `.env.example` placeholder (see below) |
| `APP_NAME`, `APP_ENV`, `DEBUG` | `Granary`, `development`, `false` | |
| `APP_URL` | `http://localhost:3000` | Public URL: links in emails, and the frontend's `NEXTAUTH_URL` |
| `API_URL` | `http://localhost:8000` | Keep `http://backend:8000` in Docker. The browser uses its own origin; a source build may still bake `NEXT_PUBLIC_API_URL` in |
| `DATABASE_URL`, `POSTGRES_PASSWORD` | required | `POSTGRES_HOST`, `POSTGRES_PORT`, `POSTGRES_DB`, `POSTGRES_USER` default to `db`, `5432`, `granary`, `granary` |
| `REDIS_PASSWORD` | required by Compose | The Redis container's password; `REDIS_URL` and `CELERY_BROKER_URL` must carry it |
| `REDIS_URL` | `redis://redis:6379/0` | Cache, pub/sub, login limiter, leases |
| `CELERY_BROKER_URL`, `CELERY_RESULT_BACKEND` | Redis databases 1 and 2 | The background job queue (the names are historical) |
| `JWT_SECRET_KEY` | required, at least 32 characters | Signs the backend's access and refresh tokens. The backend refuses to start while it starts with the `.env.example` placeholder (see below) |
| `JWT_ALGORITHM`, `JWT_ACCESS_TOKEN_EXPIRE_MINUTES`, `JWT_REFRESH_TOKEN_EXPIRE_DAYS`, `JWT_REFRESH_GRACE_SECONDS` | `HS256`, `60`, `30`, `60` | The grace period keeps a just-rotated refresh token usable for racing requests (0 = off) |
| `NEXTAUTH_SECRET` | required by Compose | The frontend's session key, its own value. When the backend sees it, a placeholder value stops it too |
| `INTERNAL_AUTH_TOKEN` | empty | Shared by the frontend and the backend: the frontend passes the visitor's address for login throttling with it |
| `SIGNAL_TICKET_KEY` | required by Compose | Signs video-call tickets (backend and signal service) |
| `SUPERADMIN_EMAIL`, `SUPERADMIN_NAME` | `owner@example.com`, `Robert Smith` | The superadmin created on the first start |
| `SUPERADMIN_PASSWORD` | required | Its initial password |
| `MINIO_SECRET_KEY` | required | The S3 storage (SeaweedFS in docker-compose; the names date from the earlier MinIO setup). Also `MINIO_ENDPOINT` (`storage:9000`), `MINIO_ACCESS_KEY` (`granary`), `MINIO_BUCKET_DOCUMENTS` (`granary-documents`), `MINIO_BUCKET_ASSETS` (`granary-assets`), `MINIO_BUCKET_PUBLIC` (`granary-public`, the only publicly readable bucket), `MINIO_USE_SSL` (`false`) |
| `MINIO_PUBLIC_URL` | `http://localhost:9000` | Where browsers reach the storage; in production the `/minio` path on your domain |
| `WHATSAPP_SERVICE_URL`, `WHATSAPP_SERVICE_TOKEN`, `WHATSAPP_BUSINESS_NUMBER` | `http://whatsapp:3001`, empty, empty | A WhatsApp bridge you provide (section 8) |
| `PDF_SERVICE_URL`, `PDF_SERVICE_TOKEN` | `http://pdf:3002`, empty | `PDF_ASSET_HOSTS`: extra hosts the renderer may load images from |
| `FACE_SERVICE_URL`, `FACE_SERVICE_TOKEN` | `http://face:3003`, empty | `FACE_MAX_CONCURRENT`: analyses run at once. The face service's own `HOST`: unset listens on all interfaces; `127.0.0.1` on bare metal |
| `RESEARCH_SERVICE_URL`, `RESEARCH_SERVICE_TOKEN` | `http://research:3004`, empty | `SEARXNG_SECRET` is read by the searxng container |
| `STT_SERVICE_URL`, `STT_SERVICE_TOKEN` | `http://stt:3005`, empty | `STT_MODEL` (`small`), `STT_CPUS` (`1.5`), `STT_THREADS` (`1`) are read by Compose |
| `SIGN_CERT_P12_PATH`, `SIGN_CERT_P12_PASSWORD`, `SIGN_TSA_URL` | `/keys/platform.p12`, empty, empty | Section 15 |
| `SIGN_SEAL_NAME`, `SIGN_REASON`, `SIGN_LOCATION` | `Your Company Ltd`, `Sealed via Granary e-signature`, `Tokyo` | Text inside the PAdES seal: set your own |
| `SIGN_QES_ENABLED`, `SIGN_QTSP_URL`, `SIGN_QTSP_API_KEY` | `false`, empty, empty | Reserved: no remote signing provider is wired up |
| `SIGNAL_SERVICE_URL`, `SIGNAL_URL` | `ws://signal:3003`, empty | `SIGNAL_URL` is the public WebSocket URL (`wss://your-domain/ws/signal`), also the frontend build argument |
| `TURN_SECRET`, `TURN_URIS` | `change_me_turn_secret` (logs a warning), localhost URIs | Section 14. `TURN_EXTERNAL_IP` is read by coturn |
| `GOOGLE_AUTH_URL`, `GOOGLE_TOKEN_URL`, `GOOGLE_API_BASE` | Google's public endpoints | Calendar integration for meetings; leave as they are |
| `EXCHANGERATE_API_KEY` | empty | Optional; used for the automatic FX sync before the key saved in Settings |
| `CORS_ORIGINS` | `["http://localhost:3000"]` | JSON list |
| `BIND_ADDR` | `0.0.0.0:8000` | Where `granary-api` listens (Compose sets it) |

**Placeholder secrets stop the start.** The API (`granary-api`) and the worker refuse to start while
`JWT_SECRET_KEY`, `APP_SECRET_KEY` or `NEXTAUTH_SECRET` (when it is in their environment or `.env`) still starts
with the `.env.example` placeholder (`change-this`, also written `change_this` or `changethis`): anyone who has
read that file could sign sessions and tokens with it. The log names the variable (never its value). Generate a
value for each, a different one per variable, with `openssl rand -hex 64`, put it in `.env` and start again.
Changing `JWT_SECRET_KEY` signs everyone out; set `APP_SECRET_KEY` once, before the first start with real data,
because encrypted settings can't be read with another key. Other weak secrets (short keys, `test...`,
`password...`, placeholder passwords) don't stop the start: they are logged and the Owner is alerted once per
boot.

Used by Compose or the frontend only: `INTERNAL_API_URL` (the frontend server's route to the backend; keep
`http://backend:8000`), `TURN_EXTERNAL_IP` (coturn), `SEARXNG_SECRET` (searxng), `STT_*` limits. The `.env.example`
file comments each one. `WWEBJS_WEB_VERSION` and the other `WA_*` variables in `docker-compose.yml` only matter
to a WhatsApp bridge, which Granary does not ship.

Business behaviour (rates, limits, schedules, time zone) is never set through `.env`; use the settings in
section 17.
