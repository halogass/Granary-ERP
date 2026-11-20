# Contributing to Granary

Thank you for helping. This page covers the development setup, the tests, and the checks a change must pass
before it is merged. Continuous integration (`.github/workflows/ci.yml`) runs the same checks on every pull
request.

## Repository layout

| Path | What it is |
|---|---|
| `rust-version/` | The Rust workspace: `crates/api` (HTTP API), `crates/worker` (background jobs), `crates/services` (business logic), `crates/core` (config, money, settings, auth), `crates/db` (migrations, seed), `crates/compat`, and the sidecars in `services/` (pdf, face, signal, research) |
| `rust-version/compat-tests/` | Golden files for the golden tests, and the test environment (`env.test.sh`) |
| `frontend/` | The Next.js app |
| `services/stt/` | The speech-to-text sidecar (Python, faster-whisper) |
| `infra/` | Nginx, coturn and SearXNG configuration |
| `scripts/` | The repository hygiene gate |
| `docs/` | Operator documentation |

## Development setup

You need:

- Rust stable with `rustfmt` and `clippy` (`rustup component add rustfmt clippy`);
- Node.js 20 and npm;
- Docker, for the test database, Redis and the GreenMail mail server the tests use;
- Python 3 for `scripts/hygiene.py` (standard library only) and, if you work on it, `services/stt`.

### Test database and Redis

The tests read their connection settings from `rust-version/compat-tests/env.test.sh`: PostgreSQL with pgvector
on `127.0.0.1:55433` (user `granary`, password `granarytest`, database `granary_test`), Redis on
`127.0.0.1:56380`, and a second Redis on `127.0.0.1:56381` for the API tests (so their Redis databases never
collide with the services tests'). Start matching containers once:

```bash
docker run -d --name granary-test-pg \
  -e POSTGRES_USER=granary -e POSTGRES_PASSWORD=granarytest -e POSTGRES_DB=granary_test \
  -p 127.0.0.1:55433:5432 pgvector/pgvector:pg16
docker run -d --name granary-test-redis -p 127.0.0.1:56380:6379 redis:7-alpine
docker run -d --name granary-test-redis-api -p 127.0.0.1:56381:6379 redis:7-alpine redis-server --databases 64
```

The database tests create a fresh database per test, so the `granary` role needs to be allowed to create
databases (the container's own user is a superuser). The IMAP and SMTP tests need a GreenMail mail server on
ports 53143 (IMAP) and 53025 (SMTP); the services tests start one when it isn't running, the API tests expect it:

```bash
docker run -d --name granary-test-greenmail -p 127.0.0.1:53025:3025 -p 127.0.0.1:53143:3143 \
  -e GREENMAIL_OPTS="-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled" \
  greenmail/standalone:2.1.2
```

### Running the app from source

The API and the worker read their configuration from the environment and from a `.env` file in the working
directory (the variables are described in [Configuration](docs/CONFIGURATION.md#19-environment-variables)).
`granary-migrate` and `granary-seed` take the database from `DATABASE_URL` in the environment (or
`--database-url`). With PostgreSQL and Redis running and a `.env` in `rust-version/`:

```bash
cd rust-version
export DATABASE_URL=postgres://granary:<password>@127.0.0.1:5432/granary
cargo run -p granary-db --bin granary-migrate    # apply the migrations
cargo run -p granary-db --bin granary-seed       # default data, superadmin
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

Or run the whole stack with `docker compose up -d --build` as in the [README](README.md#quick-start).

`rust-version/Cargo.toml` keeps `debug = "line-tables-only"` for dev and test builds: backtraces keep file and
line, and `target/` stays much smaller. Please keep it.

## Running the tests

### Rust

```bash
cd rust-version
source compat-tests/env.test.sh
cargo test --workspace --no-fail-fast
```

`./ci.sh` runs the full local gate in order (formatting, clippy, the workspace tests, then each golden test on
its own) and prints a summary; logs go to `target/ci-logs`. To run one test file:
`cargo test -p granary-services --test fx_golden`.

`rust-version/.cargo/config.toml` caps test threads (`RUST_TEST_THREADS=4`, each database test holds its own
database and pool) and raises the test thread stack to 8 MiB. Lower the thread count if your machine runs out
of memory.

### Golden files

Golden tests (`crates/*/tests/*golden*.rs`) compare the output of the code with recorded files under
`rust-version/compat-tests/golden/`. When you change behaviour on purpose, re-record the affected files:

```bash
GOLDEN_BLESS=1 cargo test -p <crate> --test <golden_test>
git diff rust-version/compat-tests/golden/     # review every change before committing it
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

### Speech-to-text service

```bash
cd services/stt
pip install -r requirements.txt
pytest -q               # the model is replaced in the tests: nothing is downloaded
```

## Formatting and lints

```bash
cd rust-version
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Clippy runs with `-D warnings`: any lint fails the build. Workspace settings are in `rust-version/clippy.toml`.
A deliberate exception is an `#[allow(...)]` at the site with a one-line reason.

## Hygiene gate

CI runs three more checks on every pull request. Run them before you push:

- **Content and commits:** `python3 scripts/hygiene.py --commits origin/main..HEAD`. It checks every tracked
  text file against a private denylist (stored as salted hashes in `scripts/hygiene-denylist.txt`, with path
  exceptions in `scripts/hygiene-allow.txt`) and refuses commits that carry AI-assistant attribution trailers.
- **Secrets:** `gitleaks dir . --config .gitleaks.toml`. The allow-list covers test fixtures only.
- **Dependency licences:** `cargo deny check licenses` in `rust-version/`, against the allowed licences in
  `rust-version/deny.toml`. A new dependency must use one of them.

## No secrets in the repository

Never commit a real key, password, token or certificate, not even in a test. Secrets belong in `.env` (which
is ignored by git) or encrypted in the application's settings. Tests use obvious fixed test values, and the
example values in `.env.example` are placeholders. If you commit a secret by mistake, treat it as leaked:
rotate it, then remove it from the history.

## Pull requests

- Keep a pull request to one change, with tests for new behaviour.
- Business numbers (rates, limits, schedules) are settings, never literals in the code: add a seeded setting
  with a default and a place in the Settings UI.
- Write user-facing text in plain English.

## Licence of contributions

Granary is dual-licensed under MIT OR Apache-2.0. As [NOTICE](NOTICE) states, unless you explicitly state
otherwise, any contribution you intentionally submit for inclusion in Granary, as defined in the Apache-2.0
license, is dual licensed as above, without any additional terms or conditions. No contributor licence
agreement or sign-off line is needed.
