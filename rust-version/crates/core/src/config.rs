//! backend/app/config.py: the same environment variables, defaults and validators, loaded once.
//!
//! pydantic-settings semantics kept: names are case-insensitive, real environment variables win over the
//! `.env` file in the working directory, `List[str]` values are JSON, ints/bools are parsed like Pydantic
//! lax mode, unknown variables are ignored (`extra="ignore"`). Field names keep the Python spelling so
//! `settings.JWT_SECRET_KEY` reads the same in both code bases.

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::Arc;

use once_cell::sync::OnceCell;

use crate::validation::{Cx, PyObj, Validate};

/// A value type a setting can have.
trait FromSetting: Sized {
    fn from_setting(raw: &str) -> Result<Self, String>;
}

impl FromSetting for String {
    fn from_setting(raw: &str) -> Result<Self, String> {
        Ok(raw.to_string())
    }
}

impl FromSetting for i64 {
    fn from_setting(raw: &str) -> Result<Self, String> {
        let mut cx = Cx::new();
        i64::validate(&PyObj::str(raw), &mut cx).ok_or_else(|| cx.errors[0].msg.clone())
    }
}

impl FromSetting for bool {
    fn from_setting(raw: &str) -> Result<Self, String> {
        let mut cx = Cx::new();
        bool::validate(&PyObj::str(raw), &mut cx).ok_or_else(|| cx.errors[0].msg.clone())
    }
}

impl FromSetting for Vec<String> {
    /// Complex fields are JSON-decoded by pydantic-settings before validation.
    fn from_setting(raw: &str) -> Result<Self, String> {
        let v = crate::validation::json_loads(raw.as_bytes())
            .map_err(|_| "error parsing value (not valid JSON)".to_string())?;
        let mut cx = Cx::new();
        Vec::<String>::validate(&v, &mut cx).ok_or_else(|| cx.errors[0].msg.clone())
    }
}

macro_rules! settings {
    ( $( $name:ident : $ty:ty $(= $default:expr)? ),* $(,)? ) => {
        /// `app.config.Settings`.
        #[derive(Clone, Debug)]
        pub struct Settings {
            $( pub $name: $ty, )*
        }

        impl Settings {
            /// Builds the settings from a case-insensitive name -> value map.
            pub fn from_map(values: &HashMap<String, String>) -> Result<Settings, SettingsError> {
                let mut errors: Vec<String> = Vec::new();
                $(
                    let $name: Option<$ty> = match values.get(&stringify!($name).to_ascii_lowercase()) {
                        Some(raw) => match <$ty as FromSetting>::from_setting(raw) {
                            Ok(v) => Some(v),
                            Err(e) => { errors.push(format!("{}\n  {}", stringify!($name), e)); None }
                        },
                        None => settings!(@default $name, errors $(, $default)?),
                    };
                )*
                if !errors.is_empty() {
                    return Err(SettingsError(errors));
                }
                let s = Settings { $( $name: $name.expect("checked"), )* };
                s.validate()
            }
        }
    };
    (@default $name:ident, $errors:ident) => {{
        $errors.push(format!("{}\n  Field required", stringify!($name)));
        None
    }};
    (@default $name:ident, $errors:ident, $default:expr) => {
        Some($default.into())
    };
}

settings! {
    // App
    APP_NAME: String = "Granary",
    APP_ENV: String = "development",
    APP_SECRET_KEY: String,
    APP_URL: String = "http://localhost:3000",
    API_URL: String = "http://localhost:8000",
    DEBUG: bool = false,

    // Database
    DATABASE_URL: String,
    POSTGRES_HOST: String = "db",
    POSTGRES_PORT: i64 = 5432i64,
    POSTGRES_DB: String = "granary",
    POSTGRES_USER: String = "granary",
    POSTGRES_PASSWORD: String,

    // Redis
    REDIS_URL: String = "redis://redis:6379/0",
    CELERY_BROKER_URL: String = "redis://redis:6379/1",
    CELERY_RESULT_BACKEND: String = "redis://redis:6379/2",

    // JWT
    JWT_SECRET_KEY: String,
    JWT_ALGORITHM: String = "HS256",
    JWT_ACCESS_TOKEN_EXPIRE_MINUTES: i64 = 60i64,
    JWT_REFRESH_TOKEN_EXPIRE_DAYS: i64 = 30i64,
    JWT_REFRESH_GRACE_SECONDS: i64 = 60i64,

    // Superadmin (seeded on first run)
    SUPERADMIN_EMAIL: String = "owner@example.com",
    SUPERADMIN_NAME: String = "Robert Smith",
    SUPERADMIN_PASSWORD: String,

    // MinIO
    MINIO_ENDPOINT: String = "storage:9000",
    MINIO_ACCESS_KEY: String = "granary",
    MINIO_SECRET_KEY: String,
    MINIO_BUCKET_DOCUMENTS: String = "granary-documents",
    MINIO_BUCKET_ASSETS: String = "granary-assets",
    MINIO_USE_SSL: bool = false,
    MINIO_PUBLIC_URL: String = "http://localhost:9000",
    // security 26 Oct: the one bucket anyone may read (branding, website product images); the others are private
    MINIO_BUCKET_PUBLIC: String = "granary-public",

    // WhatsApp service
    WHATSAPP_SERVICE_URL: String = "http://whatsapp:3001",
    WHATSAPP_SERVICE_TOKEN: String = "",
    WHATSAPP_BUSINESS_NUMBER: String = "",

    // PDF / face / research / STT services
    PDF_SERVICE_URL: String = "http://pdf:3002",
    PDF_SERVICE_TOKEN: String = "",
    FACE_SERVICE_URL: String = "http://face:3003",
    FACE_SERVICE_TOKEN: String = "",
    RESEARCH_SERVICE_URL: String = "http://research:3004",
    RESEARCH_SERVICE_TOKEN: String = "",
    STT_SERVICE_URL: String = "http://stt:3005",
    STT_SERVICE_TOKEN: String = "",
    GOOGLE_AUTH_URL: String = "https://accounts.google.com/o/oauth2/v2/auth",
    GOOGLE_TOKEN_URL: String = "https://oauth2.googleapis.com/token",
    GOOGLE_API_BASE: String = "https://www.googleapis.com",

    // Digital signature (PAdES platform seal)
    SIGN_CERT_P12_PATH: String = "/keys/platform.p12",
    SIGN_CERT_P12_PASSWORD: String = "",
    SIGN_TSA_URL: String = "",
    SIGN_SEAL_NAME: String = "Your Company Ltd",
    SIGN_REASON: String = "Sealed via Granary e-signature",
    SIGN_LOCATION: String = "Tokyo",
    SIGN_QES_ENABLED: bool = false,
    SIGN_QTSP_URL: String = "",
    SIGN_QTSP_API_KEY: String = "",

    // WebRTC signal service
    SIGNAL_SERVICE_URL: String = "ws://signal:3003",
    SIGNAL_URL: String = "",

    // TURN/STUN
    TURN_SECRET: String = "change_me_turn_secret",
    TURN_URIS: Vec<String> = vec!["turn:localhost:3478".to_string(), "turns:localhost:5349".to_string()],

    // ExchangeRate-API
    EXCHANGERATE_API_KEY: String = "",

    // CORS
    CORS_ORIGINS: Vec<String> = vec!["http://localhost:3000".to_string()],
}

/// pydantic `ValidationError` raised while building Settings (the app refuses to start).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError(pub Vec<String>);

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} validation error{} for Settings",
            self.0.len(),
            if self.0.len() == 1 { "" } else { "s" }
        )?;
        for e in &self.0 {
            write!(f, "\n{e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for SettingsError {}

impl Settings {
    /// The `@field_validator`s.
    fn validate(self) -> Result<Settings, SettingsError> {
        if self.JWT_SECRET_KEY.chars().count() < 32 {
            return Err(SettingsError(vec![
                "JWT_SECRET_KEY\n  Value error, JWT_SECRET_KEY must be at least 32 characters"
                    .into(),
            ]));
        }
        if self.TURN_SECRET == "change_me_turn_secret" {
            tracing::warn!(
                "TURN_SECRET is set to the default value: change it before deploying to production"
            );
        }
        Ok(self)
    }

    /// `.env` in the working directory, overlaid by the process environment.
    pub fn from_env() -> Result<Settings, SettingsError> {
        let mut values: HashMap<String, String> = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(".env") {
            for (k, v) in parse_dotenv(&text) {
                values.insert(k.to_ascii_lowercase(), v);
            }
        }
        for (k, v) in std::env::vars() {
            values.insert(k.to_ascii_lowercase(), v);
        }
        Settings::from_map(&values)
    }

    /// `allow_origins` of the CORS middleware: env list plus the public site, order kept, duplicates
    /// dropped (`list(dict.fromkeys([...]))`).
    pub fn cors_allow_origins(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for o in self
            .CORS_ORIGINS
            .iter()
            .map(String::as_str)
            .chain(PUBLIC_SITE_ORIGINS)
        {
            if !out.iter().any(|x| x == o) {
                out.push(o.to_string());
            }
        }
        out
    }

    /// The JWT parameters for granary_compat's jose port.
    pub fn jwt(&self) -> granary_compat::crypto::jwt::JwtConfig {
        let mut c = granary_compat::crypto::jwt::JwtConfig::new(self.JWT_SECRET_KEY.clone());
        c.algorithm = self.JWT_ALGORITHM.clone();
        c.access_minutes = self.JWT_ACCESS_TOKEN_EXPIRE_MINUTES;
        c.refresh_days = self.JWT_REFRESH_TOKEN_EXPIRE_DAYS;
        c
    }
}

/// `PUBLIC_SITE_ORIGINS` in backend/app/main.py.
pub const PUBLIC_SITE_ORIGINS: [&str; 2] = ["https://example.com", "https://www.example.com"];

/// Minimal python-dotenv: `KEY=VALUE`, optional `export `, quotes, `#` comments on unquoted values.
pub fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim().to_string();
        let v = v.trim();
        let v = if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
            || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
        {
            let inner = &v[1..v.len() - 1];
            if v.starts_with('"') {
                inner.replace("\\n", "\n").replace("\\\"", "\"")
            } else {
                inner.to_string()
            }
        } else {
            match v.find(" #") {
                Some(i) => v[..i].trim_end().to_string(),
                None => v.to_string(),
            }
        };
        out.push((k, v));
    }
    out
}

static SETTINGS: OnceCell<Arc<Settings>> = OnceCell::new();

/// `get_settings()`: loaded on first use; a configuration error aborts like the Python import would.
pub fn settings() -> Arc<Settings> {
    SETTINGS
        .get_or_init(|| {
            Arc::new(Settings::from_env().unwrap_or_else(|e| panic!("invalid configuration: {e}")))
        })
        .clone()
}

/// Installs explicit settings (tests, tools). Returns false if settings were already loaded.
pub fn init_settings(s: Settings) -> bool {
    SETTINGS.set(Arc::new(s)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> HashMap<String, String> {
        [
            ("app_secret_key", "s"),
            ("database_url", "postgresql://x"),
            ("postgres_password", "p"),
            ("jwt_secret_key", "0123456789abcdef0123456789abcdef"),
            ("superadmin_password", "p"),
            ("minio_secret_key", "m"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn defaults_and_overrides() {
        let mut m = base();
        m.insert(
            "cors_origins".into(),
            r#"["https://erp.example.com","https://example.com"]"#.into(),
        );
        m.insert("debug".into(), "yes".into());
        m.insert("postgres_port".into(), " 6543 ".into());
        let s = Settings::from_map(&m).unwrap();
        assert_eq!(s.APP_NAME, "Granary");
        assert!(s.DEBUG);
        assert_eq!(s.POSTGRES_PORT, 6543);
        assert_eq!(s.JWT_ACCESS_TOKEN_EXPIRE_MINUTES, 60);
        assert_eq!(
            s.TURN_URIS,
            vec!["turn:localhost:3478", "turns:localhost:5349"]
        );
        assert_eq!(
            s.cors_allow_origins(),
            vec![
                "https://erp.example.com",
                "https://example.com",
                "https://www.example.com"
            ]
        );
    }

    #[test]
    fn validators() {
        let mut m = base();
        m.insert("jwt_secret_key".into(), "short".into());
        let e = Settings::from_map(&m).unwrap_err();
        assert!(e
            .to_string()
            .contains("JWT_SECRET_KEY must be at least 32 characters"));
        let mut m = base();
        m.remove("database_url");
        m.insert("cors_origins".into(), "not json".into());
        let e = Settings::from_map(&m).unwrap_err();
        assert_eq!(e.0.len(), 2);
    }

    #[test]
    fn dotenv() {
        let v = parse_dotenv("# c\nA=1\nexport B=\"x y\"\nC=z # note\nD='q'\n");
        assert_eq!(
            v,
            vec![
                ("A".into(), "1".into()),
                ("B".into(), "x y".into()),
                ("C".into(), "z".into()),
                ("D".into(), "q".into())
            ]
        );
    }
}
