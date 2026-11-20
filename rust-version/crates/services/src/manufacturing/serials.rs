//! Generated production-unit serials (3 Nov 2026, Owner decisions; migration 0151).
//!
//! ```text
//!     PREFIX-XXXXX-XXXXC        e.g. DSK-7K2M9-QXH48, a test unit DSKT-7K2M9-QXH48
//! ```
//!
//! - PREFIX: the hardware product's `serial_prefix` (2 to 6 letters; DSK for the HW-DSK-* products, SOV for
//!   HW-SOV-*), plus `T` for a test unit. A product without a prefix is refused (no default).
//! - The first 9 characters are the leading 45 random bits of a fresh UUID v4 in Crockford base32
//!   (`0123456789ABCDEFGHJKMNPQRSTVWXYZ`, no I L O U); that UUID is the unit's id. The serial is only its readable
//!   form and is never decoded back.
//! - C is the Crockford check symbol: the 45-bit value mod 37 over the alphabet extended with `*~$=U`.
//!
//! Typed or searched serials are normalised (case, dashes and spaces dropped, I and L read as 1, O as 0: the SQL
//! function `production_serial_key` does the same and carries the unique index). A serial in the generated format
//! whose check symbol does not match is refused as a typo; old free-form serials (DSK-TEST-000001) work unchanged.

use granary_core::auth::AuthUser;
use granary_core::settings_store::require_int;
use granary_core::{ApiError, ApiResult};
use serde_json::{json, Value};
use sqlx::PgConnection;
use uuid::Uuid;

/// Crockford base32.
pub const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// The check symbols: the alphabet and `*~$=U` for 32 to 36.
pub const CHECK_SYMBOLS: &[u8; 37] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ*~$=U";
/// Characters of the random part.
pub const BODY_LEN: usize = 9;
/// Random bits in a serial.
pub const BITS: u32 = 45;
/// How many draws before generating gives up (a clash on the normalised serial draws again).
pub const TRIES_KEY: &str = "manufacturing.serial.generate_tries";
pub const SETTINGS_WHERE: &str = "Operations → Manufacturing";
/// The longest serial a unit keeps (production_units.serial_number is VARCHAR(100)).
pub const MAX_LEN: usize = 100;
pub const TYPO: &str = "This serial's check character does not match: probably a typo";
pub const PREFIX_RULE: &str = "The serial prefix must be 2 to 6 letters (A to Z), e.g. DSK";

/// The leading 45 bits of a UUID.
pub fn bits_of(id: Uuid) -> u64 {
    (id.as_u128() >> (128 - BITS)) as u64
}

/// The 9 Crockford characters of a 45-bit value, most significant first.
pub fn encode(value: u64) -> String {
    (0..BODY_LEN)
        .rev()
        .map(|i| ALPHABET[((value >> (5 * i)) & 31) as usize] as char)
        .collect()
}

/// The value of Crockford characters (already normalised); None for a character outside the alphabet.
pub fn decode(s: &str) -> Option<u64> {
    s.bytes().try_fold(0u64, |acc, b| {
        let d = ALPHABET.iter().position(|&a| a == b)? as u64;
        Some((acc << 5) | d)
    })
}

/// The Crockford check symbol of a value.
pub fn check_symbol(value: u64) -> char {
    CHECK_SYMBOLS[(value % 37) as usize] as char
}

/// The serial of a unit: `PREFIX[T]-XXXXX-XXXXC`.
pub fn format_serial(prefix: &str, is_test: bool, id: Uuid) -> String {
    let v = bits_of(id);
    let body = encode(v);
    format!(
        "{prefix}{}-{}-{}{}",
        if is_test { "T" } else { "" },
        &body[..5],
        &body[5..],
        check_symbol(v)
    )
}

/// The normalised form (the SQL `production_serial_key` for ASCII): upper case, no dashes or whitespace, I and L
/// read as 1, O as 0.
pub fn normalize(raw: &str) -> String {
    raw.chars()
        .filter(|c| *c != '-' && !c.is_ascii_whitespace())
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        })
        .collect()
}

/// A product's serial prefix as typed: trimmed and upper-cased; empty is None; else 2 to 6 letters.
pub fn normalize_prefix(raw: &str) -> Result<Option<String>, &'static str> {
    let p = raw.trim().to_ascii_uppercase();
    if p.is_empty() {
        return Ok(None);
    }
    if (2..=6).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_uppercase()) {
        Ok(Some(p))
    } else {
        Err(PREFIX_RULE)
    }
}

/// What a typed serial is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// Not in the generated format (an old free-form serial, or part of one).
    FreeForm,
    /// The generated format with a matching check symbol: the canonical spelling.
    Valid(String),
    /// The generated format, but the check symbol does not match.
    Typo,
}

/// Reads a typed serial against the known prefixes (each also with `T`, a test unit). When the input has dashes
/// or spaces they must group as PREFIX, 5, 5 (so DSK-2026-000123 stays free-form); without them the length says.
pub fn check(raw: &str, prefixes: &[String]) -> Check {
    let norm = normalize(raw);
    let groups: Vec<&str> = raw
        .trim()
        .split(|c: char| c == '-' || c.is_ascii_whitespace())
        .filter(|g| !g.is_empty())
        .collect();
    let grouped = groups.len() > 1;
    if grouped && (groups.len() != 3 || groups[1].len() != 5 || groups[2].len() != 5) {
        return Check::FreeForm;
    }
    let mut seen = false;
    for p in prefixes {
        for pre in [p.clone(), format!("{p}T")] {
            let pn = normalize(&pre);
            if grouped && normalize(groups[0]) != pn {
                continue;
            }
            if norm.len() != pn.len() + BODY_LEN + 1 || !norm.starts_with(&pn) {
                continue;
            }
            let rest = &norm[pn.len()..];
            let Some(value) = decode(&rest[..BODY_LEN]) else {
                continue;
            };
            let c = rest.as_bytes()[BODY_LEN];
            if !CHECK_SYMBOLS.contains(&c) {
                continue;
            }
            seen = true;
            if check_symbol(value) as u8 == c {
                return Check::Valid(format!(
                    "{pre}-{}-{}{}",
                    &rest[..5],
                    &rest[5..BODY_LEN],
                    c as char
                ));
            }
        }
    }
    if seen {
        Check::Typo
    } else {
        Check::FreeForm
    }
}

fn typo() -> ApiError {
    ApiError::app(422, TYPO, Some("serial_typo"))
}

/// Every product's serial prefix (deleted products too: their units keep their serials).
pub async fn prefixes(db: &mut PgConnection) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT serial_prefix FROM hardware_products WHERE serial_prefix IS NOT NULL ORDER BY 1",
    )
    .fetch_all(&mut *db)
    .await?)
}

/// The unit whose normalised serial equals this one's (other than `except`): (id, serial).
pub async fn unit_with(
    db: &mut PgConnection,
    serial: &str,
    except: Option<Uuid>,
) -> ApiResult<Option<(Uuid, String)>> {
    Ok(sqlx::query_as(
        "SELECT id, serial_number FROM production_units \
         WHERE production_serial_key(serial_number) = production_serial_key($1) \
           AND ($2::uuid IS NULL OR id <> $2) LIMIT 1",
    )
    .bind(serial)
    .bind(except)
    .fetch_optional(&mut *db)
    .await?)
}

/// A searched or looked-up serial: a typo (generated format, wrong check symbol) is refused unless a unit has
/// exactly that serial (an old free-form one that happens to look generated).
pub async fn guard_lookup(db: &mut PgConnection, raw: &str) -> ApiResult<()> {
    let known = prefixes(db).await?;
    if check(raw, &known) == Check::Typo && unit_with(db, raw, None).await?.is_none() {
        return Err(typo());
    }
    Ok(())
}

/// A serial typed for a unit (registering with a given serial, or relabelling): trimmed, at most 100 characters,
/// the check symbol right when it is in the generated format (stored in its canonical spelling), and no other unit
/// with the same normalised serial (409). Returns what to store.
pub async fn typed_serial(
    db: &mut PgConnection,
    raw: &str,
    except: Option<Uuid>,
) -> ApiResult<String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(ApiError::http(422, "The serial number is empty"));
    }
    if s.chars().count() > MAX_LEN {
        return Err(ApiError::http(
            422,
            format!("The serial number is longer than {MAX_LEN} characters"),
        ));
    }
    let known = prefixes(db).await?;
    let serial = match check(s, &known) {
        Check::Valid(c) => c,
        Check::FreeForm => s.to_string(),
        Check::Typo => {
            // an existing unit with exactly this spelling keeps it (its own serial, saved again)
            match unit_with(db, s, None).await? {
                Some((id, _)) if Some(id) == except => s.to_string(),
                _ => return Err(typo()),
            }
        }
    };
    if let Some((_, other)) = unit_with(db, &serial, except).await? {
        return Err(ApiError::app(
            409,
            format!(
                "The serial {serial} is already used by another unit ({other}): each unit needs its own serial"
            ),
            Some("serial_taken"),
        ));
    }
    Ok(serial)
}

/// A new unit's id and serial for a hardware product: draws fresh UUIDs (from `draw`) until the serial is free,
/// at most `manufacturing.serial.generate_tries` times. A product without a serial prefix is refused.
pub async fn generate_with(
    db: &mut PgConnection,
    product_id: Uuid,
    is_test: bool,
    draw: &mut (dyn FnMut() -> Uuid + Send),
) -> ApiResult<(Uuid, String)> {
    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT name, sku, serial_prefix FROM hardware_products WHERE id = $1 AND is_deleted = false",
    )
    .bind(product_id)
    .fetch_optional(&mut *db)
    .await?;
    let Some((name, sku, prefix)) = row else {
        return Err(ApiError::not_found(
            "Product",
            Some(&product_id.to_string()),
        ));
    };
    let Some(prefix) = prefix.filter(|p| !p.is_empty()) else {
        return Err(ApiError::app(
            422,
            format!(
                "The hardware product {name} ({sku}) has no serial prefix, so the ERP cannot generate its units' \
                 serials: set one on the product (Manufacturing > Products, Serial prefix, e.g. DSK)"
            ),
            Some("no_serial_prefix"),
        ));
    };
    let tries = require_int(db, TRIES_KEY, SETTINGS_WHERE).await?;
    if !(1..=50).contains(&tries) {
        return Err(ApiError::http(
            400,
            format!("{TRIES_KEY} must be between 1 and 50 (Settings → {SETTINGS_WHERE})"),
        ));
    }
    for _ in 0..tries {
        let id = draw();
        let serial = format_serial(&prefix, is_test, id);
        let id_taken: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM production_units WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *db)
                .await?;
        if id_taken.is_none() && unit_with(db, &serial, None).await?.is_none() {
            return Ok((id, serial));
        }
    }
    Err(ApiError::app(
        409,
        format!("No free serial was found in {tries} tries: please try again"),
        Some("serial_exhausted"),
    ))
}

/// [`generate_with`] with fresh random UUIDs.
pub async fn generate(
    db: &mut PgConnection,
    product_id: Uuid,
    is_test: bool,
) -> ApiResult<(Uuid, String)> {
    generate_with(db, product_id, is_test, &mut Uuid::new_v4).await
}

/// Relabels a unit (any time; audit-logged): the typed serial after [`typed_serial`].
pub async fn set_serial(
    db: &mut PgConnection,
    user: &AuthUser,
    unit_id: Uuid,
    raw: &str,
) -> ApiResult<Value> {
    let row: Option<String> =
        sqlx::query_scalar("SELECT serial_number FROM production_units WHERE id = $1 FOR UPDATE")
            .bind(unit_id)
            .fetch_optional(&mut *db)
            .await?;
    let Some(old) = row else {
        return Err(ApiError::not_found(
            "ProductionUnit",
            Some(&unit_id.to_string()),
        ));
    };
    let serial = typed_serial(db, raw, Some(unit_id)).await?;
    if serial != old {
        sqlx::query(
            "UPDATE production_units SET serial_number = $2, updated_at = now() WHERE id = $1",
        )
        .bind(unit_id)
        .bind(&serial)
        .execute(&mut *db)
        .await?;
        sqlx::query(
            "INSERT INTO audit_logs (id, user_id, user_name, action, entity_type, entity_id, entity_label, \
             old_values, new_values, origin) VALUES ($1, $2, $3, 'update', 'production_unit', $4, $5, $6::json, \
             $7::json, 'ui')",
        )
        .bind(Uuid::new_v4())
        .bind(user.id)
        .bind(&user.name)
        .bind(unit_id.to_string())
        .bind(crate::nary::py_slice(&serial, 200))
        .bind(json!({"serial_number": old}))
        .bind(json!({"serial_number": serial}))
        .execute(&mut *db)
        .await?;
    }
    Ok(json!({
        "id": unit_id.to_string(),
        "serial_number": serial,
        "previous_serial_number": old,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Vec<String> {
        vec!["DSK".into(), "SOV".into()]
    }

    #[test]
    fn crockford_vectors() {
        assert_eq!(encode(0), "000000000");
        assert_eq!(check_symbol(0), '0');
        // Crockford's own example: 1234 is 16J, check 1234 mod 37 = 13 = D
        assert_eq!(encode(1234), "00000016J");
        assert_eq!(check_symbol(1234), 'D');
        assert_eq!(decode("00000016J"), Some(1234));
        assert_eq!(encode((1 << 45) - 1), "ZZZZZZZZZ");
        assert_eq!(decode("ZZZZZZZZZ"), Some((1 << 45) - 1));
        // 32..36 take the extra symbols
        for (v, c) in [(32u64, '*'), (33, '~'), (34, '$'), (35, '='), (36, 'U')] {
            assert_eq!(check_symbol(v), c);
            assert_eq!(check_symbol(v + 37), c);
        }
        assert_eq!(check_symbol(31), 'Z');
        assert_eq!(check_symbol(37), '0');
        // no I L O U in the body alphabet
        for b in ALPHABET {
            assert!(!b"ILOU".contains(b));
        }
        assert_eq!(decode("0000000I0"), None);
    }

    #[test]
    fn the_serial_is_the_leading_45_bits() {
        let id = Uuid::parse_str("ffffffff-ffff-4fff-bfff-ffffffffffff").unwrap();
        assert_eq!(bits_of(id), (1 << 45) - 1);
        let id = Uuid::parse_str("00000000-0000-4000-8000-000000000000").unwrap();
        assert_eq!(format_serial("DSK", false, id), "DSK-00000-00000");
        // 0x0000_0000_2690 << 3 >> 3: the top 48 bits are 0x000000002690, so 45 bits = 0x4D2 = 1234
        let id = Uuid::parse_str("00000000-2690-4000-8000-000000000000").unwrap();
        assert_eq!(bits_of(id), 1234);
        assert_eq!(format_serial("SOV", true, id), "SOVT-00000-016JD");
        let s = format_serial("DSK", false, Uuid::new_v4());
        assert_eq!(s.len(), 15);
        assert!(s.starts_with("DSK-"));
        assert_eq!(check(&s, &p()), Check::Valid(s.clone()));
    }

    #[test]
    fn normalisation() {
        assert_eq!(normalize(" dsk-7k2m9-qxh48 "), "DSK7K2M9QXH48");
        assert_eq!(normalize("DSK 7K2M9 QXH48"), "DSK7K2M9QXH48");
        assert_eq!(normalize("iLo-O"), "1100");
        assert_eq!(normalize("DSK-TEST-000001"), "DSKTEST000001");
        assert_eq!(normalize("SOV"), "S0V");
    }

    #[test]
    fn typos_are_caught_and_old_serials_pass() {
        let id = Uuid::parse_str("00000000-2690-4000-8000-000000000000").unwrap();
        let good = format_serial("DSK", false, id); // DSK-00000-016JD
        assert_eq!(good, "DSK-00000-016JD");
        // case, dashes, spaces and O for 0 / I for 1 still read
        assert_eq!(check("dsk00000016jd", &p()), Check::Valid(good.clone()));
        assert_eq!(check("DSK-OOOOO-OI6JD", &p()), Check::Valid(good.clone()));
        assert_eq!(check("dsk 00000 016jd", &p()), Check::Valid(good.clone()));
        assert_eq!(
            check("sov-00000-016jd", &p()),
            Check::Valid("SOV-00000-016JD".into())
        );
        assert_eq!(
            check("DSKT-00000-016JD", &p()),
            Check::Valid("DSKT-00000-016JD".into())
        );
        // one character wrong: the check symbol does not match
        assert_eq!(check("DSK-00000-016KD", &p()), Check::Typo);
        assert_eq!(check("DSK-00000-016JE", &p()), Check::Typo);
        // free-form serials
        assert_eq!(check("DSK-TEST-000001", &p()), Check::FreeForm);
        assert_eq!(check("DSK-2026-000123", &p()), Check::FreeForm);
        assert_eq!(check("ACME-LAP-2026-000001", &p()), Check::FreeForm);
        assert_eq!(check("ZZZ-00000-016KD", &p()), Check::FreeForm);
        assert_eq!(check("016JD", &p()), Check::FreeForm);
        // an unknown prefix list: nothing is generated
        assert_eq!(check("DSK-00000-016KD", &[]), Check::FreeForm);
    }

    #[test]
    fn prefixes() {
        assert_eq!(normalize_prefix(" dsk "), Ok(Some("DSK".into())));
        assert_eq!(normalize_prefix(""), Ok(None));
        assert_eq!(normalize_prefix("SOVEREIGN"), Err(PREFIX_RULE));
        assert_eq!(normalize_prefix("A"), Err(PREFIX_RULE));
        assert_eq!(normalize_prefix("AB1"), Err(PREFIX_RULE));
    }
}
