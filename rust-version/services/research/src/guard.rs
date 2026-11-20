//! Port of services/research/app/guard.py.
//!
//! Which addresses the research service may talk to: the public internet, nothing else.
//!
//! Company websites are typed in by users, so every URL is hostile until proven otherwise: a
//! "website" of http://db:5432 or http://169.254.169.254 must never be fetched. The check is on
//! the *resolved IP*, and the caller connects to that same IP, so a DNS answer that changes
//! between the check and the connection (rebinding) cannot slip through.

use std::net::IpAddr;

use crate::{pyip, pyurl};

pub const ALLOWED_PORTS: [u32; 2] = [80, 443];

/// Why a check failed. `Blocked` is guard.py's `Blocked(ValueError)`; `Internal` is any other
/// exception Python would let escape (a ValueError from `urlsplit`/`.port`, a UnicodeError from the
/// IDNA codec): the route answers 500 for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardError {
    Blocked(String),
    Internal(String),
}

impl GuardError {
    fn blocked(s: impl Into<String>) -> Self {
        GuardError::Blocked(s.into())
    }
}

/// `ip_is_public(ip)` for a string Python would accept (`None` otherwise).
pub fn ip_is_public(ip: &str) -> Option<bool> {
    pyip::ip_is_public(ip)
}

/// The host as `socket.getaddrinfo` sends it to the resolver: an ASCII str goes as is, anything else
/// through the IDNA codec (here the `idna` crate's UTS 46 mapping, see the report for edge cases).
fn resolver_host(host: &str) -> Result<String, GuardError> {
    if host.contains('\0') {
        return Err(GuardError::Internal("embedded null character".into()));
    }
    if host.is_ascii() {
        return Ok(host.to_string());
    }
    idna::domain_to_ascii(host)
        .map_err(|e| GuardError::Internal(format!("encoding with 'idna' codec failed: {e:?}")))
}

/// `getaddrinfo(host, port, type=SOCK_STREAM)` through the system resolver (glibc, as Python).
pub async fn getaddrinfo(host: &str, port: u16) -> Result<Vec<IpAddr>, GuardError> {
    let h = resolver_host(host)?;
    match tokio::net::lookup_host((h.clone(), port)).await {
        Ok(addrs) => Ok(addrs.map(|a| a.ip()).collect()),
        Err(_) => Ok(Vec::new()),
    }
}

/// `resolve_public(host, port)`: one public IP to connect to, or Blocked.
pub async fn resolve_public(host: &str, port: u32) -> Result<IpAddr, GuardError> {
    if host.is_empty() {
        return Err(GuardError::blocked("No host in the address."));
    }
    // Python passes the int port to getaddrinfo; every caller has checked it is 80 or 443.
    let ips = getaddrinfo(host, port as u16).await?;
    if ips.is_empty() {
        return Err(GuardError::blocked(format!("{host} does not resolve.")));
    }
    // Every answer must be public: a host that resolves to one public and one private address
    // is exactly how a rebinding attack is set up.
    if !ips.iter().all(|ip| pyip::is_public((*ip).into())) {
        return Err(GuardError::blocked(format!(
            "{host} points to a private or internal address."
        )));
    }
    Ok(ips[0])
}

/// `check_url(url)`: syntactic check. Returns (scheme, host, port); the IP check happens at connect time.
pub fn check_url(url: &str) -> Result<(String, String, u32), GuardError> {
    let parts = pyurl::urlsplit(url).map_err(GuardError::Internal)?;
    let scheme = parts.scheme.to_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(GuardError::blocked(
            "Only http and https addresses can be researched.",
        ));
    }
    if parts.username().is_some_and(|u| !u.is_empty())
        || parts.password().is_some_and(|p| !p.is_empty())
    {
        return Err(GuardError::blocked(
            "Addresses with credentials are not fetched.",
        ));
    }
    let hostname = parts.hostname().unwrap_or_default();
    let host = granary_compat::text::pystr::lower(hostname.trim_matches('.'));
    if host.is_empty() {
        return Err(GuardError::blocked("No host in the address."));
    }
    let port = match parts.port().map_err(GuardError::Internal)? {
        Some(p) if p != 0 => p,
        _ => {
            if scheme == "https" {
                443
            } else {
                80
            }
        }
    };
    if !ALLOWED_PORTS.contains(&port) {
        return Err(GuardError::blocked(
            "Only the standard web ports (80, 443) are allowed.",
        ));
    }
    if let Some(public) = pyip::ip_is_public(&host) {
        if !public {
            return Err(GuardError::blocked("That address is private or internal."));
        }
    }
    Ok((scheme, host, port))
}

/// `check_url_resolved(url)`: full check for a URL we are about to fetch ourselves.
pub async fn check_url_resolved(url: &str) -> Result<IpAddr, GuardError> {
    let (_, host, port) = check_url(url)?;
    resolve_public(&host, port).await
}
