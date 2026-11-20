# Architecture

## System diagram

```
                        +-----------------------------------+
Browser / phone         |  Nginx (on the host, apt package) |
                ------->|  :80 / :443                       |
                        +---+---------------+------------+--+
                            | localhost     | localhost  | localhost
                 +----------v--+     +------v---------+  +v---------------+
                 | frontend    |     | backend        |  | signal         |
                 | Next.js 14  |     | granary-api    |  | WebRTC         |  <- Docker containers,
                 | :3000       |     | :8000          |  | signalling     |     host ports bound to
                 +-------------+     +--+--------+----+  | :3003          |     127.0.0.1 only
                                        |        |       +----------------+
                      +-----------------v-+   +--v----------+
                      | PostgreSQL 16     |   | Redis 7     |  <- Docker (internal network)
                      | + pgvector        |   | cache, job  |
                      +-------------------+   | queue, pub/ |
                                              | sub, leases |
                                              +--+----------+
                                                 |
                                   +-------------v------------+
                                   | worker + beat            |  <- same image as backend,
                                   | granary-worker           |     two containers
                                   +--------------------------+

   Internal sidecars (Docker network only, no host port):
   +-- pdf       pdf:3002       Rust + headless Chromium (document templates)
   +-- face      face:3003      Rust + ONNX Runtime (face matching, passive liveness)
   +-- research  research:3004  Rust + Moli page reader, behind a filtering proxy ("research" network)
   +-- searxng   searxng:8080   self-hosted meta search ("research" network)
   +-- stt       stt:3005       Python + faster-whisper (voice notes, on the CPU)

   Object storage (Docker):
   +-- storage   127.0.0.1:9000 (SeaweedFS S3 API, proxied read-only at /minio/)

   Video calls (host network):
   +-- coturn    TURN/STUN :3478 / :5349, relay ports 49152-65535/udp
```

Nginx runs on the host, not in Docker. It listens on 80/443 and proxies to the containers, which publish their
ports on `127.0.0.1` only.

Granary does not ship a WhatsApp Web bridge. The backend talks to one at `WHATSAPP_SERVICE_URL` (default
`http://whatsapp:3001`) when you provide it; without it, the WhatsApp inbox and Nary's WhatsApp channel stay
disconnected.

---

## Request routing (Nginx, `infra/nginx/granary.conf`)

| Path | Target |
|---|---|
| `/ws/signal` | signal service `:3003` (WebSocket) |
| `/api/v1/(messaging\|whatsapp\|calls)/ws` | backend, WebSocket upgrade |
| `/api/v1/ai/` | backend, unbuffered with a long read timeout (the assistant streams its replies) |
| `/api/auth/` | Next.js (NextAuth) |
| `/api/favicon`, `/api/icons/` | Next.js (dynamic favicon and PWA icons) |
| `= /api/v1/whatsapp/export` | backend, long read timeout |
| `/api/v1/documents/` | backend, 180 s read timeout (sending a document renders its PDF and waits for the mail server) |
| `/api/v1/public/drop/` | backend, 600 s timeouts (public file drop links) |
| `/.well-known/oauth-*`, `/api/v1/mcp/oauth/` | backend (MCP browser login), login rate limit |
| `/api/v1/auth/login` | backend, stricter rate limit (5 requests/min per address, burst 10) |
| `/api/` | backend `:8000` (30 requests/s per address, burst 50) |
| `/health` | backend health check |
| `/minio/` | the storage `:9000` (public files and presigned links, read-only) |
| `/_next/static/` | Next.js (one-year immutable cache) |
| `/` | Next.js `:3000` |

The Next.js server rewrites `/api/v1/*` to `INTERNAL_API_URL` (`http://backend:8000`), the Docker-internal
address. Never point it at the public domain: Nginx would redirect `http://` to `https://` and the redirect
would leak to the browser.

---

## Auth and sessions

```
Browser -> POST /api/auth/callback/credentials        (NextAuth)
        -> authorize() -> POST http://backend:8000/api/v1/auth/login
        -> backend returns { access_token, refresh_token, user_id, ... }
        -> NextAuth keeps the tokens and their expiry in its own HttpOnly cookie

Each authenticated request:
        -> NextAuth jwt() callback
        -> access token close to expiry: POST /api/v1/auth/refresh, rotate tokens
        -> refresh fails (revoked or expired): session.error = "RefreshTokenExpired",
           the client signs out to /login
```

- Backend tokens: access 60 minutes, refresh 30 days (`JWT_ACCESS_TOKEN_EXPIRE_MINUTES`,
  `JWT_REFRESH_TOKEN_EXPIRE_DAYS`). A just-rotated refresh token stays valid for `JWT_REFRESH_GRACE_SECONDS`
  (60 s), so parallel requests or several tabs refreshing at once don't sign the user out.
- The NextAuth cookie is signed with `NEXTAUTH_SECRET`, which is separate from the backend's `JWT_SECRET_KEY`.
- The frontend server passes the visitor's address to the backend's login throttling in `X-Client-IP`, signed
  with `INTERNAL_AUTH_TOKEN`; the backend trusts that header only with the token, and otherwise uses Nginx's
  `X-Real-IP`.
- `GET /api/v1/auth/me` returns the user's effective permissions; the frontend hides actions the user can't take,
  and the backend checks every request again.
- A user manager who is not a system admin can't create or promote a system admin and can assign only roles
  whose permissions they hold.
- The login rate limit (`auth.login_max_attempts` per `auth.login_window_seconds`, per address, in Redis) fails
  open with a logged error when Redis or its settings are missing, so a mistake can't lock everyone out.
- Two-factor sign-in (TOTP) is available per user.

---

## User portals

All portals share one sign-in page at `/login`. After sign-in, users go where their `portal_type` says:

| Portal type | Entry point | Layout |
|---|---|---|
| `internal` | `/dashboard` | Full ERP sidebar |
| `partner` | `/partner/dashboard` | Partner portal |
| `vendor` | `/vendor/dashboard` | Vendor portal |

Partners and vendors can register themselves (`/api/v1/portal`); an admin approves them under
**Admin → Portal users**.

Public pages without sign-in: `/clock?kiosk=1` (registered kiosk tablets carry their own device token),
`/v/<token>` (QR document verification), `/sign/...` (signer invitations), `/meet/<room>` (guest video-call
join), `/unsubscribe` (email campaigns), `/drop/...` (file drop links).

---

## Tech stack

| Layer | Technology |
|---|---|
| Frontend | Next.js 14 (App Router), TypeScript, Tailwind CSS 3, NextAuth v4, PWA (Serwist service worker) |
| UI | Radix UI primitives, Lucide icons, react-markdown with remark-gfm and KaTeX (AI answers) |
| Backend | Rust: axum, tokio, sqlx (PostgreSQL), `rust_decimal` for money |
| Database | PostgreSQL 16 + pgvector (embeddings for SOP, AI-knowledge and CRM retrieval) |
| Cache, queue | Redis 7: cache, the background job queue, WebSocket fan-out (pub/sub), leases |
| Background jobs | `granary-worker worker` (runs jobs) and `granary-worker beat` (enqueues the schedule), separate containers |
| Object storage | SeaweedFS (S3-compatible; the `MINIO_*` variable names date from the earlier MinIO setup): a private documents bucket, a private assets bucket, one public bucket |
| PDF | `granary-pdf`: headless Chromium driven over CDP, Handlebars-style templates in `rust-version/services/pdf/assets` |
| Face matching | `granary-face`: ONNX Runtime; only face descriptors are stored, never photos |
| Web research | SearXNG (search) and `granary-research` with Moli (page reading) behind a filtering proxy |
| Speech to text | `services/stt`: Python, faster-whisper on the CPU; audio never leaves the server |
| AI providers | Anthropic, OpenAI and OpenAI-compatible endpoints, Gemini, DeepSeek, Ollama; chosen per job in Settings |
| E-signature | PAdES sealing of signed PDFs, optional RFC 3161 timestamps |
| Video calls | WebRTC peer to peer; `granary-signal` for signalling (HMAC-signed tickets); coturn for TURN/STUN |
| Migrations | `granary-migrate`, run by the backend container on every start; the schema starts from one baseline (`rust-version/crates/db/migrations/0001_baseline.sql`) |
| Auth | JWT (access + refresh), bcrypt, NextAuth v4, optional TOTP |
| Encryption at rest | Fernet (mail passwords, AI and API keys, other secrets in settings) |
| FX rates | ExchangeRate-API, synced every 6 hours |

---

## Nary, the AI assistant

The AI code lives in `rust-version/crates/services/src/ai/` (providers, the assistant loop, tools, knowledge,
enrichment, MCP access) and `.../src/nary/` (Nary's channels, intake, cards, meetings, work queue).
`ai::registry::get_provider()` builds the adapter configured for a job (`anthropic.rs`, `openai.rs` including
DeepSeek and OpenAI-compatible endpoints, `gemini.rs`, `ollama.rs`). The model per job is the `ai.routing`
setting; the house style (no em dashes in AI output, setting `ai.no_em_dash`) is applied to every provider.

**One assistant turn** (streamed to the browser as Server-Sent Events from `POST /api/v1/ai/assistant/chat`):

1. The closest SOPs (pgvector) and matching AI-knowledge passages go into the system prompt.
2. A tool loop of up to 8 rounds. Tools run as the user and check that user's permissions: SOP and knowledge
   search, CRM records, pipeline, deals, contacts, invoices, projects, email and chat history, web search, page
   reading, research.
3. Tools that would change data never save anything themselves: they return a **confirm card**. The user applies
   or discards it in the chat; the outcome is stored on the message
   (`PATCH /api/v1/ai/conversations/{id}/proposals/{pid}`) and given to the model on the next turn.
4. When the rounds run out, one last call without tools makes the model answer, so the user always gets a reply.

Provider errors reach the user as plain sentences (bad key, unknown model, rate limit). Usage and estimated
cost are logged per call in `ai_usage_log`; the monthly budget (`ai.budget.*`) warns, then stops non-essential
jobs.

**MCP.** The same tool registry is exposed to outside AI clients at `/api/v1/mcp` (JSON-RPC 2.0 over HTTP:
`initialize`, `tools/list`, `tools/call`, `resources/*`, `prompts/*`). A connection identifies a person with a
personal access token (`gpat_...`, Settings → Integrations & access → MCP access) or a browser login
(`gmcp_...`, OAuth). It acts with that person's permissions, narrowed by its level: `read`, `propose`
(changes come back as cards) or `act`. The Owner can grant a further level with full access, protected by
confirm codes.

---

## Documents: approvals, verification, e-signature, delivery

- **Approvals** (`services/src/approvals/`, `routers/approvals.rs`): approval policies per document type, edited
  in Settings → Approvals & signing. Steps can carry conditions such as "amount above N in the functional
  currency", "not a catalog item at list price" or "FX rate missing". Each document type has an adapter
  (`approvals/adapters.rs`: quotation, proposal, sales contract, invoice, expense, purchase order, BOM, contractor
  request, work order, vendor invoice, marketing content, hiring request, offer letter, SOP article, payroll
  run, library document version) with three hooks: on submit, on decision (an approved expense or vendor invoice posts to the ledger, for
  example) and on cancel. A step whose amount condition can't be evaluated (a purchase order with no FX rate)
  applies rather than being skipped. Overdue steps are reminded at most once a day per request.
- **Verification** (`services/src/verification/`, `routers/verification.rs`): every page of an issued document
  carries a QR that opens `/v/<token>`, with the frozen facts and the approval record.
- **E-signature** (`routers/signing.rs`, `routers/signing_flow.rs`): quotations, invoices, purchase orders and work
  orders are not e-signed; the approver is stamped on the PDF. Contracts and offer letters use counterparty
  signing, and the final PDF is PAdES-sealed with the platform certificate. A signer's link is their credential,
  so it is returned only to that signer; signature images are PNG data limited by `sign.signature_max_bytes`.
- **Delivery** (`routers/delivery.rs`, `services/src/verification/delivery.rs`): "Send" emails the PDF from a CRM
  inbox (or sends it over WhatsApp when a bridge is connected), only once the document is approved. Emails are
  recorded as a conversation in the CRM inbox and copied to the mailbox's Sent folder, so the customer's reply
  threads onto them. A send still running can't be started twice, and the same document to the same recipient
  within 15 minutes needs a deliberate "Send again". Invoices can carry a billing email that is used before the
  CRM contact's.

---

## Money, settings and business dates

- **Money** (`crates/core/src/money.rs`): `rust_decimal` everywhere, no floats. `quantize_money` rounds half up
  to the currency's places; `compute_lines` returns document totals that always equal the sum of the rounded
  lines. The places come from the `currency.decimals` setting (`settings_store::currency_places`).
- **Business numbers are settings** (`crates/core/src/settings_store.rs`): tax defaults, payroll contributions
  and income tax, overtime rules, approval limits, reminder schedules, retry limits, tolerances, currency
  decimals, login limits and lease TTLs are `system_settings` rows, seeded with defaults and edited in Settings.
  Code reads them with `require_decimals`, `require_int`, `require_json` and `currency_places`; a missing or
  malformed value is a 400 naming the setting and where to fix it, never a fallback number. (The login rate
  limiter is the deliberate exception: it fails open.)
- **Business dates** (`crates/core/src/clock.rs`): `today_local` and `now_local` use the `timezone` setting
  (default `UTC`). Document numbers, default document dates, overdue checks, FX lookups, payroll, CRM close
  dates, daily caps and monthly resets use it; technical timestamps (`created_at`, token expiry) and the job
  schedule stay in UTC.
- **Redis leases** (`services/src/redis_lease.rs`): acquire, renew and release with a holder token, so a loop
  that runs in several processes has one active holder. Used by the email campaign sender and the personal
  mailbox sync.

---

## Invoices and the ledger

**Invoice lines** (`services/src/invoicing.rs`). Every path that writes invoice lines (create, edit, revise,
credit note, proforma to commercial, quotation to invoice) goes through `write_items`; single-line changes
re-total from the stored lines with `recalculate`. Each line has a `discount_pct` and a `tax_id` from the tax
table; the tax code and rate are copied onto the line when it is saved (`resolve_line_taxes`), so a later rate
change never rewrites an issued invoice. The server computes the header totals and the per-tax breakdown.

**Quotation to invoice.** Only sent, accepted or signed quotations can be invoiced, under a row lock. Optional
lines are not billed; a header discount becomes one negative "Discount" line; the convert dialog picks one tax for
every line (the default per currency comes from `finance.default_sales_tax`). A full invoice requires that
nothing is billed yet; a milestone can't be billed twice or push the billed total past the quotation's total. A
foreign-currency quotation with no FX rate is refused.

**Status changes** go through `invoice_transition_allowed` (`routers/finance/invoices.rs`). A commercial invoice
is cancelled only by a credit note; a proforma ends as `converted`. Moving to `sent` needs the approval
(`approvals::engine::ensure_or_submit`).

**Automatic GL posting** (`services/src/gl_posting.rs`). Each money event posts one balanced journal entry:

| Event | Entry |
|---|---|
| Commercial invoice sent | Dr AR / Cr revenue / Cr output tax; payments already made on its proforma move from customer advances to AR |
| Credit note issued | Mirror of the invoice, at the original invoice's rate |
| Payment recorded | Dr bank / Cr AR (commercial) or customer advances (proforma); FX difference to `fx_gain_loss` |
| Payment deleted | Reversal of its entry (`reverse_for`) |
| Refund | Dr AR / Cr bank, with FX |
| Vendor invoice approved, then paid | Dr vendor bill expense + input tax / Cr AP + withholding tax payable; then Dr AP / Cr bank |
| Expense approved, then reimbursed | Dr expense (per-category role) / Cr employee payable (+ withholding tax payable); then Dr employee payable / Cr bank |
| Payroll paid | Dr salaries + employer contributions / Cr income tax payable, contributions payable, bank (net pay) |
| Depreciation, asset disposal | In the functional currency, at the asset's acquisition rate |

- **Accounts come from roles.** Each line names a role (`ar`, `revenue`, `sales_tax_output`, `ap`,
  `bank_default`, `expense.<category>`, ...); finance maps roles to accounts (Finance → Chart of Accounts →
  Account mapping, `GET/PUT /api/v1/finance/gl-roles`). A bank account's own GL account overrides `bank_default`.
  An unmapped or inactive role is a 400 naming it, and the action does not half-complete.
- **Cut-over.** Posting runs only for documents dated on or after `finance.gl_autopost_from` (blank = off).
  Earlier documents are covered once by `POST /api/v1/finance/accounting/migrate-opening`.
- **Idempotent.** One entry per source document and event, enforced by a unique index; posting twice returns
  the existing entry.
- **Currencies.** Lines in the document currency balance among themselves, and functional-currency lines (FX
  differences) balance among themselves. The fiscal period must be open.
- **Reversals** keep the original entry posted with `reversed_at` set.

**Bank reconciliation.** A statement line can be matched only to a posted journal line on the bank's GL account
with the same amount that is not matched elsewhere. A statement is reconciled only when every line is matched
or excluded and opening + money in - money out equals the closing balance.

---

## Multi-currency

Invoices, expenses and payments carry a `currency` (ISO 4217) and an `exchange_rate` to the functional currency
at the time of the transaction. The functional currency is the `currency.functional` setting (default USD,
Settings → Money & tax → Currencies); it can't be changed once journal entries are posted. Supported currencies:
USD, EUR, GBP, JPY, SGD, AUD, CNY. Decimal places per currency come from `currency.decimals` (JPY 0, the others
2).

`services/src/fx.rs` fetches rates from ExchangeRate-API (key from `EXCHANGERATE_API_KEY`, or the one saved in
Settings) and stores them in `exchange_rates`. The job `sync_exchange_rates` runs every 6 hours; admins can also
**Sync Now**. Lookups take the closest rate on or before the transaction date, and the inverse pair also counts.
Business paths use `require_rate`, which turns a missing rate into a 400 instead of a silent 1.0. When a
foreign-currency invoice is paid at a different rate, the difference goes to the `fx_gain_loss` role in the
payment's ledger entry.

`GET /api/v1/system/currencies` returns the active and functional currencies to every signed-in user (forms need
them). Changing them (`PATCH`) and syncing (`POST /api/v1/system/currencies/sync`) need an admin; the API key is
stored encrypted and never returned.

---

## Background jobs

`granary-worker worker` takes jobs from a Redis queue (`CELERY_BROKER_URL`; the name is kept from an earlier
version, the queue is Granary's own). `granary-worker beat` enqueues the schedule in
`rust-version/crates/worker/src/beat.rs`; only one beat process holds the schedule at a time (a Redis lease).

| Job | Every | Purpose |
|---|---|---|
| `poll_crm_inbox` | 2 min | New mail in the CRM inboxes (Inbox and Sent) and read flags |
| `send_scheduled_messages` | 5 min | Due scheduled email and WhatsApp messages (`crm.scheduled.*`) |
| `sweep_stuck_pdf_imports` | 5 min | Fail knowledge-base PDF imports that were lost |
| `fire_due_reminders` | 5 min | Deliver due CRM follow-up reminders |
| `sync_personal_mailboxes` | 5 min | Personal mailboxes: Inbox, Sent, read flags set elsewhere |
| `nary_tick` | 5 min | Nary's briefs, heartbeat and card expiry |
| `nary_fail_stuck_cards` | 5 min | Fail Nary cards stuck while applying |
| `nary_dispatch_events` | 1 min | Hand Nary's domain events (new messages, web forms, leads, deals) to its subscribers |
| `nary_channels_tick` | 1 min | Nary's channels: owed acknowledgements, takeover check-ins, channel health checks, history import |
| `nary_playbooks_tick` | 1 min | Nary's sales playbooks: meeting reminders and briefs, the follow-up cadence, unsigned-contract nudges |
| `nary_work_queue_nudge` | 1 min | Remind the Owner of work waiting while Nary is paused |
| `gitlab_issue_sync_tick` | 1 min | R&D backlog and GitLab issues, both ways |
| `remind_expiring_signatures` | 1 h | Remind signers near the deadline (`sign.reminder_*`) |
| `remind_overdue_approvals` | 1 h | Remind approvers of overdue steps (once a day each) |
| `propose_lead_merges` | 1 h | Once a day: two open leads of the same person become a merge card for the Owner |
| `expire_quotations` | 1 h | Expire quotations past their validity, and nudge about expired ones |
| `generate_followup_reminders` | 1 h | No-reply and stale-record rules, once a day in company time |
| `weekly_followup_digest` | 1 h | The Owner's follow-up digest on the configured weekday and hour |
| `refresh_stale_ai_fields` | 1 h | Re-research stale companies once a day (CRM enrichment) |
| `erp_guide_sync` | 1 h | Keep the ERP guide in AI knowledge in step with the code and settings |
| `sync_exchange_rates` | 6 h | FX rates from ExchangeRate-API |
| `send_invoice_reminders` | 1 day | Mark overdue invoices and send reminders (`finance.invoice_reminder_*`) |
| `reindex_kb_embeddings` | 1 day | Re-embed published SOPs for AI retrieval |
| `nary_style_refresh` | 1 day | Propose a refreshed writing style guide to the Owner every `nary.style.refresh_days` |
| `remind_document_reviews` | 1 day | Remind owners of library documents due for review |

Jobs that read business settings log a missing value and skip that run rather than guess. Routers also queue
one-off jobs (sending a document by email, importing a knowledge-base PDF, enrichment, research, indexing an AI
knowledge document).

The email campaign sender is not a scheduled job: it is a loop inside every backend process, and only the
process holding the Redis lease `lock:marketing.email_queue_worker` sends. Pacing and the daily limit are
settings (`marketing.email_min_delay_seconds` and `marketing.email_max_delay_seconds`, default 30 to 60 s;
`marketing.email_daily_limit`, default 100 per day in company time).

---

## Source layout

```
granary/
+-- rust-version/                 Rust workspace (see rust-version/README.md)
|   +-- crates/
|   |   +-- api/                  granary-api: src/routers/ (one module per area, mounted at /api/v1)
|   |   +-- worker/               granary-worker: queue, beat schedule, job registry, jobs
|   |   +-- services/             business logic: ai/, nary/, approvals/, verification/, crm/,
|   |   |                         invoicing.rs, gl_posting.rs, fx.rs, numbering.rs, redis_lease.rs, ...
|   |   +-- core/                 config.rs, money.rs, settings_store.rs, clock.rs, auth.rs, errors.rs
|   |   +-- db/                   migrations/0001_baseline.sql, granary-migrate, seed/ (granary-seed)
|   |   +-- compat/               crypto (Fernet, passwords, JWT), JSON and text helpers
|   +-- services/                 sidecars: pdf/ (templates in assets/), face/, signal/, research/
|   +-- compat-tests/             golden files and the test environment
|   +-- vendor/                   patched sqlx-postgres
|   +-- Dockerfile                one target per runtime image
|   +-- ci.sh                     local CI: fmt, clippy, tests, golden tests
+-- frontend/
|   +-- src/
|   |   +-- app/
|   |   |   +-- (erp)/            ERP pages inside the sidebar layout: dashboard, assistant, approvals, crm,
|   |   |   |                     sales, finance, hr, projects, manufacturing, fleet, maintenance, helpdesk,
|   |   |   |                     marketing, partner-mgmt, vendor-mgmt, rnd, knowledge, messaging, email,
|   |   |   |                     calls, nary, sign, admin, profile, settings
|   |   |   +-- (partner)/, (vendor)/   portal layouts
|   |   |   +-- (auth)/           the sign-in page
|   |   |   +-- clock/, v/, sign/, meet/, drop/, unsubscribe/, offline/, oauth/
|   |   |   +-- api/              NextAuth handler, dynamic favicon and PWA icons
|   |   +-- components/           ui/ (shared components), ai/, crm/, settings/, layout/, ...
|   |   +-- lib/                  api.ts (API client), auth.ts (NextAuth options), settings-registry.ts, ...
|   +-- docs/UI_COMPONENTS.md     UI component guide
|   +-- next.config.mjs           rewrites /api/v1/* to INTERNAL_API_URL
+-- services/stt/                 speech-to-text sidecar (Python)
+-- infra/                        nginx/granary.conf, coturn/turnserver.conf, searxng/settings.yml
+-- scripts/                      hygiene gate (hygiene.py)
+-- docs/                         this documentation
+-- docker-compose.yml
+-- .env.example
```
