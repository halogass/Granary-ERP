//! Python's `ipaddress` module (CPython 3.12.15, the python:3.12-slim image the service runs in): which
//! strings `ip_address()` accepts, and the address classes guard.py reads (`is_private`, `is_loopback`,
//! `is_link_local`, `is_multicast`, `is_reserved`, `is_unspecified`, `ipv4_mapped`, `sixtofour`).

/// An address `ipaddress.ip_address()` accepted. The IPv6 scope id (`%eth0`) is parsed and dropped:
/// none of the properties guard.py reads depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyIp {
    V4(u32),
    V6(u128),
}

/// `ipaddress.ip_address(s)`: `None` where Python raises ValueError.
pub fn ip_address(s: &str) -> Option<PyIp> {
    if let Some(v4) = ipv4_from_str(s) {
        return Some(PyIp::V4(v4));
    }
    ipv6_from_str(s).map(PyIp::V6)
}

/// `IPv4Address(s)`.
pub fn ipv4_from_str(s: &str) -> Option<u32> {
    if s.is_empty() || s.contains('/') {
        return None;
    }
    let octets: Vec<&str> = s.split('.').collect();
    if octets.len() != 4 {
        return None;
    }
    let mut out: u32 = 0;
    for o in octets {
        out = (out << 8) | parse_octet(o)?;
    }
    Some(out)
}

fn parse_octet(o: &str) -> Option<u32> {
    if o.is_empty() || !o.bytes().all(|b| b.is_ascii_digit()) || o.len() > 3 {
        return None;
    }
    if o != "0" && o.starts_with('0') {
        return None;
    }
    let v: u32 = o.parse().ok()?;
    (v <= 255).then_some(v)
}

/// `IPv6Address(s)` (scope id split off first, as `_split_scope_id` does).
pub fn ipv6_from_str(s: &str) -> Option<u128> {
    if s.contains('/') {
        return None;
    }
    let addr = match s.split_once('%') {
        None => s,
        Some((addr, scope)) => {
            if scope.is_empty() || scope.contains('%') {
                return None;
            }
            addr
        }
    };
    ipv6_int(addr)
}

/// `_BaseV6._ip_int_from_string`.
fn ipv6_int(ip: &str) -> Option<u128> {
    const HEXTETS: usize = 8;
    if ip.is_empty() || ip.chars().count() > 45 {
        return None;
    }
    // split(':', maxsplit=9)
    let mut parts: Vec<String> = ip.splitn(HEXTETS + 2, ':').map(str::to_string).collect();
    if parts.len() < 3 {
        return None;
    }
    if parts.last().is_some_and(|p| p.contains('.')) {
        let v4 = ipv4_from_str(&parts.pop().unwrap_or_default())?;
        parts.push(format!("{:x}", (v4 >> 16) & 0xFFFF));
        parts.push(format!("{:x}", v4 & 0xFFFF));
    }
    if parts.len() > HEXTETS + 1 {
        return None;
    }
    let mut skip_index: Option<usize> = None;
    for (i, p) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if p.is_empty() {
            if skip_index.is_some() {
                return None;
            }
            skip_index = Some(i);
        }
    }
    let (parts_hi, parts_lo, parts_skipped);
    if let Some(skip) = skip_index {
        let mut hi = skip;
        let mut lo = parts.len() - skip - 1;
        if parts[0].is_empty() {
            hi -= 1;
            if hi != 0 {
                return None;
            }
        }
        if parts[parts.len() - 1].is_empty() {
            lo -= 1;
            if lo != 0 {
                return None;
            }
        }
        let skipped = HEXTETS as isize - (hi + lo) as isize;
        if skipped < 1 {
            return None;
        }
        parts_hi = hi;
        parts_lo = lo;
        parts_skipped = skipped as usize;
    } else {
        if parts.len() != HEXTETS || parts[0].is_empty() || parts[parts.len() - 1].is_empty() {
            return None;
        }
        parts_hi = parts.len();
        parts_lo = 0;
        parts_skipped = 0;
    }
    let mut v: u128 = 0;
    for p in parts.iter().take(parts_hi) {
        v = (v << 16) | parse_hextet(p)?;
    }
    v = v.checked_shl(16 * parts_skipped as u32).unwrap_or(0);
    for p in &parts[parts.len() - parts_lo..] {
        v = (v << 16) | parse_hextet(p)?;
    }
    Some(v)
}

fn parse_hextet(h: &str) -> Option<u128> {
    if h.is_empty() || h.len() > 4 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(h, 16).ok()
}

fn in4(a: u32, net: u32, prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    a & mask == net
}

fn in6(a: u128, net: u128, prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    a & mask == net
}

const fn v4(a: u8, b: u8, c: u8, d: u8) -> u32 {
    ((a as u32) << 24) | ((b as u32) << 16) | ((c as u32) << 8) | d as u32
}

const V4_PRIVATE: &[(u32, u32)] = &[
    (v4(0, 0, 0, 0), 8),
    (v4(10, 0, 0, 0), 8),
    (v4(127, 0, 0, 0), 8),
    (v4(169, 254, 0, 0), 16),
    (v4(172, 16, 0, 0), 12),
    (v4(192, 0, 0, 0), 24),
    (v4(192, 0, 0, 170), 31),
    (v4(192, 0, 2, 0), 24),
    (v4(192, 168, 0, 0), 16),
    (v4(198, 18, 0, 0), 15),
    (v4(198, 51, 100, 0), 24),
    (v4(203, 0, 113, 0), 24),
    (v4(240, 0, 0, 0), 4),
    (v4(255, 255, 255, 255), 32),
];
const V4_PRIVATE_EXCEPTIONS: &[(u32, u32)] = &[(v4(192, 0, 0, 9), 32), (v4(192, 0, 0, 10), 32)];

const V6_PRIVATE: &[(u128, u32)] = &[
    (0x1, 128),
    (0x0, 128),
    (0xffff_0000_0000, 96),
    (0x0064_ff9b_0001 << 80, 48),
    (0x0100 << 112, 64),
    (0x2001 << 112, 23),
    (0x2001_0db8 << 96, 32),
    (0x2002 << 112, 16),
    (0x3fff << 112, 20),
    (0xfc00 << 112, 7),
    (0xfe80 << 112, 10),
];
const V6_PRIVATE_EXCEPTIONS: &[(u128, u32)] = &[
    ((0x2001_0001 << 96) | 1, 128),
    ((0x2001_0001 << 96) | 2, 128),
    (0x2001_0003 << 96, 32),
    (0x2001_0004_0112 << 80, 48),
    (0x2001_0020 << 96, 28),
    (0x2001_0030 << 96, 28),
];
const V6_RESERVED: &[(u128, u32)] = &[
    (0x0, 8),
    (0x0100 << 112, 8),
    (0x0200 << 112, 7),
    (0x0400 << 112, 6),
    (0x0800 << 112, 5),
    (0x1000 << 112, 4),
    (0x4000 << 112, 3),
    (0x6000 << 112, 3),
    (0x8000 << 112, 3),
    (0xA000 << 112, 3),
    (0xC000 << 112, 3),
    (0xE000 << 112, 4),
    (0xF000 << 112, 5),
    (0xF800 << 112, 6),
    (0xFE00 << 112, 9),
];

fn v4_is_private(a: u32) -> bool {
    V4_PRIVATE.iter().any(|&(n, p)| in4(a, n, p))
        && !V4_PRIVATE_EXCEPTIONS.iter().any(|&(n, p)| in4(a, n, p))
}

fn v4_not_public(a: u32) -> bool {
    v4_is_private(a)
        || in4(a, v4(127, 0, 0, 0), 8)
        || in4(a, v4(169, 254, 0, 0), 16)
        || in4(a, v4(224, 0, 0, 0), 4)
        || in4(a, v4(240, 0, 0, 0), 4)
        || a == 0
}

fn v6_not_public(a: u128) -> bool {
    // Mapped addresses never get here (guard.py unwraps them first).
    let private = V6_PRIVATE.iter().any(|&(n, p)| in6(a, n, p))
        && !V6_PRIVATE_EXCEPTIONS.iter().any(|&(n, p)| in6(a, n, p));
    private
        || a == 1
        || in6(a, 0xfe80 << 112, 10)
        || in6(a, 0xff00 << 112, 8)
        || V6_RESERVED.iter().any(|&(n, p)| in6(a, n, p))
        || a == 0
}

/// guard.py `_EXTRA_BLOCKED`: CGNAT, benchmarking, IETF protocol assignments (IPv4 networks; an IPv6
/// address is never "in" an IPv4 network).
fn v4_extra_blocked(a: u32) -> bool {
    in4(a, v4(100, 64, 0, 0), 10) || in4(a, v4(198, 18, 0, 0), 15) || in4(a, v4(192, 0, 0, 0), 24)
}

/// guard.py `ip_is_public` on an already parsed address.
pub fn is_public(ip: PyIp) -> bool {
    let ip = match ip {
        PyIp::V6(a) => {
            if (a >> 32) == 0xFFFF {
                PyIp::V4((a & 0xFFFF_FFFF) as u32) // ipv4_mapped
            } else if (a >> 112) == 0x2002 {
                PyIp::V4(((a >> 80) & 0xFFFF_FFFF) as u32) // sixtofour
            } else if in6(a, 0x0064_ff9b << 96, 96) {
                PyIp::V4((a & 0xFFFF_FFFF) as u32) // NAT64
            } else {
                PyIp::V6(a)
            }
        }
        v4 => v4,
    };
    match ip {
        PyIp::V4(a) => !(v4_not_public(a) || v4_extra_blocked(a)),
        PyIp::V6(a) => !v6_not_public(a),
    }
}

/// guard.py `ip_is_public(ip)`: `None` where `ipaddress.ip_address` raises ValueError.
pub fn ip_is_public(ip: &str) -> Option<bool> {
    ip_address(ip).map(is_public)
}

impl From<std::net::IpAddr> for PyIp {
    fn from(ip: std::net::IpAddr) -> Self {
        match ip {
            std::net::IpAddr::V4(a) => PyIp::V4(u32::from(a)),
            std::net::IpAddr::V6(a) => PyIp::V6(u128::from(a)),
        }
    }
}
