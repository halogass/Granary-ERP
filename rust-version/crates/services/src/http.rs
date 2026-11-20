//! The shared HTTP client (Python created an httpx.AsyncClient per call; timeouts are set per request here).

use once_cell::sync::Lazy;

static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent("granary")
        .build()
        .expect("reqwest client")
});

pub fn client() -> &'static reqwest::Client {
    &CLIENT
}
