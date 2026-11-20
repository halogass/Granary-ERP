//! `get_db()`: one database session per request, committed when the handler succeeds and rolled back when
//! it fails.
//!
//! Python runs the endpoint, then (FastAPI >= 0.106) the dependency exit before the response is sent:
//! `commit()` on success, `rollback()` and re-raise on any exception. A failure at commit (deferred
//! constraint, IntegrityError on flush) therefore replaces the success response with the error response.
//! [`db_session_layer`] does the same around every request; handlers reach the session through the
//! [`Db`] extractor. The transaction begins lazily, so requests that never touch the database never take a
//! connection.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use tokio::sync::{MappedMutexGuard, Mutex, MutexGuard};

use crate::errors::{ApiError, ErrorOutcome};
use crate::state::AppState;

/// The request's session (SQLAlchemy `AsyncSession` with autocommit off).
pub struct DbSession {
    pool: PgPool,
    tx: Mutex<Option<Transaction<'static, Postgres>>>,
}

impl DbSession {
    pub fn new(pool: PgPool) -> DbSession {
        DbSession {
            pool,
            tx: Mutex::new(None),
        }
    }

    /// The connection inside the current transaction, beginning one if needed (SQLAlchemy autobegin).
    /// Hold the guard only across the queries that need it.
    pub async fn conn(&self) -> Result<MappedMutexGuard<'_, PgConnection>, ApiError> {
        let mut guard = self.tx.lock().await;
        if guard.is_none() {
            *guard = Some(self.pool.begin().await?);
        }
        Ok(MutexGuard::map(guard, |t| {
            &mut **t.as_mut().expect("transaction begun")
        }))
    }

    /// `await db.commit()`: commits what is pending; the next query begins a new transaction.
    pub async fn commit(&self) -> Result<(), ApiError> {
        let tx = self.tx.lock().await.take();
        if let Some(tx) = tx {
            tx.commit().await?;
        }
        Ok(())
    }

    /// `await db.rollback()`.
    pub async fn rollback(&self) -> Result<(), ApiError> {
        let tx = self.tx.lock().await.take();
        if let Some(tx) = tx {
            tx.rollback().await?;
        }
        Ok(())
    }
}

/// Handler argument for the request's session.
#[derive(Clone)]
pub struct Db(pub Arc<DbSession>);

impl std::ops::Deref for Db {
    type Target = DbSession;
    fn deref(&self) -> &DbSession {
        &self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Db {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Db>()
            .cloned()
            .ok_or_else(|| ApiError::internal(anyhow::anyhow!("db_session_layer is not installed")))
    }
}

/// Middleware: gives the request a session and settles it after the handler.
pub async fn db_session_layer(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let session = Arc::new(DbSession::new(state.db.clone()));
    req.extensions_mut().insert(Db(session.clone()));
    let resp = next.run(req).await;
    if resp.extensions().get::<ErrorOutcome>().is_some() {
        let _ = session.rollback().await;
        return resp;
    }
    match session.commit().await {
        Ok(()) => resp,
        Err(e) => e.into_response(),
    }
}
