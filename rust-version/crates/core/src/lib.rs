//! granary-core: the backend's cross-cutting pieces (backend/app/config.py, backend/app/core/*, the error
//! handlers of backend/app/main.py) ported faithfully, plus Pydantic/FastAPI-compatible request
//! validation.

pub mod auth;
pub mod clock;
pub mod company;
pub mod config;
pub mod db;
pub mod errors;
pub mod http;
pub mod money;
pub mod redis_client;
pub mod settings_store;
pub mod state;
pub mod validation;

pub use errors::{ApiError, ApiResult};
pub use state::AppState;
