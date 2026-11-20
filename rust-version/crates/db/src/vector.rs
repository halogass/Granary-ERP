//! pgvector `vector(n)` column value.
//!
//! Binary wire format (pgvector `vector_send`/`vector_recv`): int16 dim, int16 unused (0),
//! then dim float4 values, all big-endian. Text format (`[1,2,3]`) is not used by sqlx.
//! Binary round trip is tested in tests/migrate_db.rs.
//! TODO wave 1: check what Python's pgvector.sqlalchemy writes for NaN/empty vectors and match it.

use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{
    PgArgumentBuffer, PgHasArrayType, PgTypeInfo, PgValueFormat, PgValueRef, Postgres,
};
use sqlx::{Decode, Encode, Type};

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Vector(pub Vec<f32>);

impl From<Vec<f32>> for Vector {
    fn from(v: Vec<f32>) -> Self {
        Vector(v)
    }
}

impl Type<Postgres> for Vector {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("vector")
    }
}

impl PgHasArrayType for Vector {
    fn array_type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("_vector")
    }
}

impl Encode<'_, Postgres> for Vector {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        let dim =
            i16::try_from(self.0.len()).map_err(|_| "vector has more than 32767 dimensions")?;
        buf.extend_from_slice(&dim.to_be_bytes());
        buf.extend_from_slice(&0i16.to_be_bytes());
        for v in &self.0 {
            buf.extend_from_slice(&v.to_be_bytes());
        }
        Ok(IsNull::No)
    }
}

impl<'r> Decode<'r, Postgres> for Vector {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        match value.format() {
            PgValueFormat::Binary => {
                let b = value.as_bytes()?;
                if b.len() < 4 {
                    return Err("vector: short buffer".into());
                }
                let dim = i16::from_be_bytes([b[0], b[1]]) as usize;
                if b.len() != 4 + dim * 4 {
                    return Err("vector: length does not match dimension".into());
                }
                Ok(Vector(
                    b[4..]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| f32::from_be_bytes(*c))
                        .collect(),
                ))
            }
            PgValueFormat::Text => {
                let s = value.as_str()?.trim();
                let inner = s
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .ok_or("vector: bad text")?;
                if inner.trim().is_empty() {
                    return Ok(Vector(Vec::new()));
                }
                Ok(Vector(
                    inner
                        .split(',')
                        .map(|p| p.trim().parse::<f32>())
                        .collect::<Result<_, _>>()?,
                ))
            }
        }
    }
}
