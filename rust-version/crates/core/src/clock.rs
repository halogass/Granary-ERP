//! backend/app/core/clock.py: business dates in the company's time zone (the `timezone` system setting,
//! e.g. Asia/Tokyo). Technical timestamps (created_at, token expiry) stay in UTC.

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use sqlx::PgConnection;

use crate::settings_store::{require_from, values, SettingError, StoreResult};

pub const TIMEZONE_KEY: &str = "timezone";
pub const WHERE: &str = "Company & branding → Regional";

/// `ZoneInfo(name)`, or the SettingError Python raises for an unknown zone.
pub fn parse_tz(name: &str) -> Result<Tz, SettingError> {
    name.parse::<Tz>().map_err(|_| {
        SettingError::new(
            TIMEZONE_KEY,
            &format!("is not a known time zone ('{name}')"),
            WHERE,
        )
    })
}

/// `local_tz(db)`.
pub async fn local_tz(db: &mut PgConnection) -> StoreResult<Tz> {
    let vals = values(db, &[TIMEZONE_KEY]).await?;
    let name = require_from(&vals, TIMEZONE_KEY, WHERE)?;
    Ok(parse_tz(&name)?)
}

/// `now_in(tz)`.
pub fn now_in(tz: Tz) -> DateTime<Tz> {
    Utc::now().with_timezone(&tz)
}

/// `today_in(tz)`.
pub fn today_in(tz: Tz) -> NaiveDate {
    now_in(tz).date_naive()
}

/// `now_local(db)`.
pub async fn now_local(db: &mut PgConnection) -> StoreResult<DateTime<Tz>> {
    Ok(now_in(local_tz(db).await?))
}

/// `today_local(db)`.
pub async fn today_local(db: &mut PgConnection) -> StoreResult<NaiveDate> {
    Ok(today_in(local_tz(db).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tz() {
        assert_eq!(parse_tz("Asia/Tokyo").unwrap(), chrono_tz::Asia::Tokyo);
        assert_eq!(
            parse_tz("Mars/Base").unwrap_err().detail,
            "Setting 'timezone' is not a known time zone ('Mars/Base'). Set it in Settings → Company & branding → Regional."
        );
        let tokyo = now_in(chrono_tz::Asia::Tokyo);
        assert_eq!(tokyo.naive_utc(), tokyo.with_timezone(&Utc).naive_utc());
    }
}
