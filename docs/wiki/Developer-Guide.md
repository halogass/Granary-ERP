# Developer Guide

<!-- nav:top -->
[Home](Home) › Developers › **Developer Guide**

**On this page:** [Architecture overview](#architecture-overview) · [Repository layout](#repository-layout) · [Development setup](#development-setup) · [Running the tests](#running-the-tests) · [Formatting, lints and the hygiene gate](#formatting-lints-and-the-hygiene-gate) · [Continuous integration](#continuous-integration) · [Contributing workflow](#contributing-workflow) · [License](#license) · [Editing the Wiki](#editing-the-wiki)
<!-- /nav:top -->

This page is for people who want to change Granary: fix a bug, add a feature or run it from source. It covers the
architecture, the repository layout, building and running, the tests, the checks every change must pass, and how
to send a contribution. The repository's `CONTRIBUTING.md` and `docs/ARCHITECTURE.md` have more detail.

---

## Architecture overview

```
Browser / phone
      |
  Nginx on the host (:80 / :443)
      |-- /            -> frontend   Next.js 14             127.0.0.1:3000
      |-- /api/        -> backend    granary-api (Rust)     127.0.0.1:8000
      |-- /ws/signal   -> signal     WebRTC signalling      127.0.0.1:3003
      |-- /minio/      -> storage    SeaweedFS (S3), read-only proxy
      |
  backend ----- PostgreSQL 16 + pgvector
     |   \----- Redis 7 (cache, job queue, pub/sub, locks)
     |
  worker + beat (granary-worker, same image as backend)

  Internal sidecars (Docker network only):
    pdf       Rust + headless Chromium (document templates)
    face      Rust + ONNX Runtime (face matching, liveness)
    research  Rust page reader behind a filtering proxy, with searxng (own network)
    stt       Python + faster-whisper (speech to text, CPU)
  Host network:
    coturn    TURN/STUN for video calls
```

Main ideas:

- **One API, many routers.** `granary-api` (axum, tokio, sqlx) mounts one router per area under `/api/v1`. Each
  route guards itself with extractors: `CurrentUser`, `InternalUser` (staff only), `RequirePermission`,
  `SystemAdmin`, `Superadmin`. The frontend hides what a user can't do; the backend checks every request again.
- **Background jobs.** `granary-worker worker` takes jobs from a Redis queue; `granary-worker beat` enqueues the
  schedule (`crates/worker/src/beat.rs`). Only one beat holds the schedule at a time.
- **Money** uses `rust_decimal` everywhere, never floats. Rounding is per currency from the `currency.decimals`
  setting; document totals always equal the sum of the rounded lines.
- **Business numbers are settings.** Code reads them with `require_decimals`, `require_int`, `require_json` and
  `currency_places` (`crates/core/src/settings_store.rs`). A missing value is a 400 that names the setting, never a
  fallback number.
- **Business dates** use the company time zone (`crates/core/src/clock.rs`); technical timestamps stay in UTC.
- **AI.** Providers, the assistant loop, tools and MCP access live in `crates/services/src/ai/`; Nary's channels,
  intake, cards and work queue in `crates/services/src/nary/`. Tools that change data return confirm cards instead
  of saving. The same tool registry is served to outside AI clients over MCP (`routers/mcp.rs`,
  `routers/mcp_oauth.rs`).
- **Documents.** Approval policies and per-document adapters (`services/src/approvals/`), verification QR pages
  (`services/src/verification/`), PAdES sealing, and delivery by email from a CRM inbox.
- **Ledger.** Each money event posts one balanced, idempotent journal entry through account roles
  (`services/src/gl_posting.rs`).
- **Sign-in.** NextAuth v4 on the frontend holds the backend's access and refresh tokens; the backend uses JWTs,
  bcrypt and optional TOTP. Secrets in settings are encrypted (Fernet).

---

## Repository layout

| Path | What it is |
|---|---|
| `rust-version/` | The Rust workspace |
| `rust-version/crates/api` | `granary-api`: `src/routers/`, one module per area |
| `rust-version/crates/worker` | `granary-worker`: queue, beat schedule, jobs |
| `rust-version/crates/services` | Business logic: `ai/`, `nary/`, `approvals/`, `verification/`, `crm/`, invoicing, GL posting, FX, ... |
| `rust-version/crates/core` | Config, money, settings store, clock, auth, errors |
| `rust-version/crates/db` | Migrations (`migrations/0001_baseline.sql` onward), `granary-migrate`, the seed (`granary-seed`) |
| `rust-version/crates/compat` | Crypto (Fernet, passwords, JWT), JSON and text helpers |
| `rust-version/services/` | Sidecars: `pdf/` (templates in `assets/`), `face/`, `signal/`, `research/` |
| `rust-version/compat-tests/` | Golden files and the test environment (`env.test.sh`) |
| `rust-version/Dockerfile` | One build target per runtime image |
| `rust-version/ci.sh` | The local CI gate |
| `frontend/` | The Next.js app: `src/app/(erp)/` (ERP pages), `(partner)/`, `(vendor)/`, `components/`, `lib/` (`api.ts`, `settings-registry.ts`) |
| `services/stt/` | Speech-to-text sidecar (Python) |
| `infra/` | Nginx, coturn and SearXNG configuration |
| `scripts/` | The hygiene gate and the CI test runner |
| `docs/` | Operator documentation; `docs/wiki/` holds these wiki pages |

---

## Development setup

You need:

- Rust stable with `rustfmt` and `clippy` (`rustup component add rustfmt clippy`);
- Node.js 20 and npm;
- Docker, for the test database, Redis and a test mail server;
- Python 3 for `scripts/hygiene.py` (standard library only) and, if you work on it, `services/stt`.

### Run from source

With PostgreSQL (with pgvector) and Redis running, and a `.env` in `rust-version/`:

```bash
cd rust-version
export DATABASE_URL=postgres://granary:<password>@127.0.0.1:5432/granary
cargo run -p granary-db --bin granary-migrate    # apply the migrations
cargo run -p granary-db --bin granary-seed       # default data and the Owner account
cargo run -p granary-api                         # API on :8000 (BIND_ADDR or PORT to change it)
cargo run -p granary-worker -- worker            # background jobs (optional)
cargo run -p granary-worker -- beat              # their schedule (optional)
```

```bash
cd frontend
npm ci
INTERNAL_API_URL=http://localhost:8000 NEXTAUTH_URL=http://localhost:3000 \
  NEXTAUTH_SECRET=<any-long-random-value> npm run dev
```

Or run the whole stack with `docker compose up -d --build` (see [Installation](Installation)).

Keep `debug = "line-tables-only"` under `[profile.dev]` in `rust-version/Cargo.toml`: backtraces keep file and
line, and `target/` stays much smaller.

---

## Running the tests

### Test containers

The tests read their connection settings from `rust-version/compat-tests/env.test.sh`. Start matching containers
once:

```bash
docker run -d --name granary-test-pg \
  -e POSTGRES_USER=granary -e POSTGRES_PASSWORD=granarytest -e POSTGRES_DB=granary_test \
  -p 127.0.0.1:55433:5432 pgvector/pgvector:pg16
docker run -d --name granary-test-redis -p 127.0.0.1:56380:6379 redis:7-alpine
docker run -d --name granary-test-redis-api -p 127.0.0.1:56381:6379 redis:7-alpine redis-server --databases 64
docker run -d --name granary-test-greenmail -p 127.0.0.1:53025:3025 -p 127.0.0.1:53143:3143 \
  -e GREENMAIL_OPTS="-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled" \
  greenmail/standalone:2.1.2
```

- PostgreSQL on port 55433: database tests create a fresh database per test, so the `granary` role must be allowed
  to create databases (the container's own user is a superuser).
- Two Redis instances: 56380 for the services tests, 56381 for the API tests, so they never collide.
- GreenMail (IMAP 53143, SMTP 53025) for the mail tests.

### Rust

```bash
cd rust-version
source compat-tests/env.test.sh
cargo test --workspace --no-fail-fast
cargo test -p granary-services --test fx_golden     # one test file
./ci.sh                                             # the full local gate, with a summary
```

`./ci.sh` runs formatting, clippy, the workspace tests and each golden test on its own; logs go to
`target/ci-logs`. `rust-version/.cargo/config.toml` caps test threads (`RUST_TEST_THREADS=4`) and raises the test
thread stack to 8 MiB; lower the thread count if you run out of memory.

Every integration test links the whole backend, so all test binaries together are very large.
`scripts/ci-rust-tests.sh` (run from `rust-version/`) builds and runs one test target at a time and deletes each
binary after it ran. `SHARD` and `SHARDS` split the targets over several machines; shard 1 also runs the unit and
doc tests. Use it when disk space is tight:

```bash
cd rust-version && source compat-tests/env.test.sh && bash ../scripts/ci-rust-tests.sh
```

### Golden tests

Golden tests (`crates/*/tests/*golden*.rs`) compare the code's output with recorded files under
`rust-version/compat-tests/golden/`. When you change behavior on purpose, re-record and review:

```bash
GOLDEN_BLESS=1 cargo test -p <crate> --test <golden_test>
git diff rust-version/compat-tests/golden/
```

Bless only after a change you meant, and say in the pull request which golden files changed and why.

### Frontend

```bash
cd frontend
npm ci
npx tsc --noEmit -p .   # types
npx eslint src          # lint
npx vitest run          # tests
```

### Speech to text

```bash
cd services/stt
pip install -r requirements.txt
pytest -q               # the model is replaced in the tests: nothing is downloaded
```

---

## Formatting, lints and the hygiene gate

```bash
cd rust-version
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Clippy runs with `-D warnings`: any lint fails the build. A deliberate exception is an `#[allow(...)]` at the site
with a one-line reason.

The **hygiene gate** runs three more checks. Run them before you push:

- **Content and commits:** `python3 scripts/hygiene.py --commits origin/main..HEAD`. It checks every tracked text
  file against a private denylist (stored as salted hashes in `scripts/hygiene-denylist.txt`, with path exceptions
  in `scripts/hygiene-allow.txt`) and refuses commits that carry AI-assistant attribution trailers.
- **Secrets:** `gitleaks dir . --config .gitleaks.toml`. The allow-list covers test fixtures only.
- **Dependency licenses:** `cargo deny check licenses` in `rust-version/`, against `rust-version/deny.toml`. A new
  dependency must use an allowed license.

---

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main` and every pull request:

| Job | What it checks |
|---|---|
| **Rust (fmt, clippy)** | `cargo fmt --check` and clippy with `-D warnings`, on a pinned toolchain |
| **Rust tests (1/4 ... 4/4)** | The test suite split over four runners with `scripts/ci-rust-tests.sh`, with PostgreSQL + pgvector, Redis and GreenMail as service containers |
| **Frontend (types, lint, tests)** | `tsc`, `eslint`, `vitest` on Node 20 |
| **Hygiene** | Content denylist and commit attribution, gitleaks secret scan, `cargo deny` licenses |

A newer pipeline run for the same branch cancels the older one.

---

## Contributing workflow

1. Fork the repository and create a branch.
2. Make one change per pull request, with tests for new behavior.
3. Business numbers (rates, limits, schedules) are settings, never literals: add a seeded setting with a default
   and a place in the Settings UI (`frontend/src/lib/settings-registry.ts`).
4. Write user-facing text in plain English.
5. Run `./ci.sh`, the frontend checks and the hygiene gate locally.
6. Open the pull request and describe what changed, including any blessed golden files.

**No secrets in the repository.** Never commit a real key, password, token or certificate, not even in a test.
Secrets belong in `.env` (ignored by git) or encrypted in Settings. If you commit one by mistake, treat it as
leaked: rotate it, then remove it from the history.

**Security problems** go through the repository's **Security** tab (**Report a vulnerability**), never a public
issue.

---

## License

Granary is dual-licensed under the MIT License or the Apache License, Version 2.0, at your option
(`LICENSE-MIT`, `LICENSE-APACHE`). Unless you explicitly state otherwise, any contribution you intentionally submit
for inclusion is dual-licensed the same way, without additional terms. No contributor license agreement or
sign-off line is needed. Third-party components keep their own licenses; see `NOTICE`.

## Editing the Wiki

The Wiki's source lives in `docs/wiki/` (one Markdown file per page; the file name is the page name). Its images
live only in the Wiki's own repository, under `images/`. The navigation (the sidebar, the breadcrumb and
"On this page" list at the top of each page, and the Previous / Next links at the bottom) is generated from one
tree in `scripts/wiki-nav.py`: after adding, renaming or reordering a page, add it to the tree there and run

```bash
python3 scripts/wiki-nav.py
```

The generated blocks sit between `<!-- nav:... -->` markers, so the script can be run again at any time.

## Related pages

- [Installation](Installation)
- [Configuration](Configuration)
- [Administration](Administration#reporting-a-security-problem)

<!-- nav:bottom -->
---

← Previous: [Troubleshooting](Troubleshooting)
<!-- /nav:bottom -->
