//! staff_chat `_as_user`'s transport (formerly a wave-4 placeholder): the real app, called in process.
use serde_json::Value;

use granary_core::auth::AuthUser;

use crate::nary::staff_chat::PyExc;

/// services/nary/staff_chat.py `_as_user`: `AsyncClient(transport=ASGITransport(app=app),
/// base_url="http://nary.internal/api/v1", headers={"Authorization": f"Bearer {create_access_token(str(user.id))}"},
/// timeout=120).request(method, path, json=json)`: the axum router called in process ([`crate::inproc`]).
/// Returns (status, body). A transport failure (no app installed, timeout) is the httpx exception.
pub async fn as_user_request(
    user: &AuthUser,
    method: &str,
    path: &str,
    json: Option<Value>,
) -> Result<(u16, Vec<u8>), PyExc> {
    let call = crate::inproc::request_as(user.id, method, path, json);
    match tokio::time::timeout(std::time::Duration::from_secs(120), call).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(PyExc::new("RuntimeError", e.to_string())),
        Err(_) => Err(PyExc::new("ReadTimeout", "")),
    }
}
