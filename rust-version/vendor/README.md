# Vendored crates

- `sqlx-postgres` 0.8.6 (crates.io, unchanged except below), wired in through `[patch.crates-io]` in the
  workspace Cargo.toml.
  - `src/types/rust_decimal.rs`: a NUMERIC zero keeps its display scale (`0.00` for NUMERIC(14,2)). Upstream
    returns `Decimal::ZERO` (scale 0) for every zero, while asyncpg gives `Decimal('0.00')`, so `str()` of a
    stored zero differed from Python. Drop this copy when upstream keeps the scale.
  - `src/lib.rs`: `#![allow(dead_code, deprecated)]` (a path dependency is not lint-capped like a registry one).
