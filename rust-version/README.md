# Granary: the Rust workspace

The backend and the Rust sidecars of Granary. See the top-level [README](../README.md) for what Granary is and
[CONTRIBUTING](../CONTRIBUTING.md) for the development setup.

| Crate | Path | What it is |
|---|---|---|
| `granary-api` | `crates/api` | The HTTP API (axum): routers under `src/routers`, mounted at `/api/v1`, plus `/health` |
| `granary-worker` | `crates/worker` | `granary-worker worker` runs background jobs from a Redis queue; `granary-worker beat` enqueues the scheduled ones (`src/beat.rs`) |
| `granary-services` | `crates/services` | Business logic: AI and Nary, approvals, invoicing and GL posting, FX, mail sync, documents, signing |
| `granary-core` | `crates/core` | Configuration from the environment, money arithmetic, required settings, business dates, auth, errors |
| `granary-db` | `crates/db` | Migrations (`migrations/`) with `granary-migrate`, and the idempotent seed (`src/seed/`) with `granary-seed` |
| `granary-compat` | `crates/compat` | Shared helpers: crypto (Fernet, password hashing), JSON and text formatting |
| `granary-pdf` | `services/pdf` | PDF rendering with headless Chromium and the document templates (`services/pdf/assets`) |
| `granary-face` | `services/face` | Face detection, matching and passive liveness (ONNX Runtime) |
| `granary-signal` | `services/signal` | WebRTC signalling for video calls |
| `granary-research` | `services/research` | Reads web pages for CRM research, behind a filtering proxy |

- `cargo check` / `cargo test` from this folder; `./ci.sh` runs fmt, clippy, the tests and the golden tests.
- `compat-tests/golden/`: recorded outputs the golden tests compare against; re-record with `GOLDEN_BLESS=1`
  after an intended change (see CONTRIBUTING).
- `Dockerfile`: one target per runtime image (`api`, `pdf`, `face`, `signal`, `research`), built by the root
  `docker-compose.yml`.
- `vendor/`: a patched copy of `sqlx-postgres` (see `vendor/README.md`).
