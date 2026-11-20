//! Request validation that behaves like Pydantic v2 (2.10 / pydantic-core 2.27.1) running under FastAPI
//! 0.115: same coercions, same English messages, same `loc` paths, same error order. The 422 body only
//! carries `loc` and `msg`, so that is what is reproduced; golden fixtures recorded from the real stack
//! live in compat-tests/golden/validation/ (recorded from the reference implementation) and are replayed by
//! tests/validation_golden.rs.
//!
//! Models are declared with [`py_model!`](crate::py_model), literals with [`py_literal!`](crate::py_literal):
//!
//! ```ignore
//! use granary_core::validation::v;
//! granary_core::py_model! {
//!     pub struct CreateItem {
//!         pub name: String => v::str_c(v::StrC::new().min_length(1).max_length(255)),
//!         pub qty: i64 = 1 => v::int_c(v::NumC::new().ge("1")),
//!         pub price: rust_decimal::Decimal,
//!         pub note: Option<String> = None,
//!         pub tags: Vec<String> = Vec::new(),
//!     }
//! }
//! ```
//!
//! A field without `= default` is required ("Field required"); `Option<T>` makes it nullable, exactly like
//! `Optional[T]` in Pydantic (nullable but still required unless it has a default). Defaults are not
//! validated, as in Pydantic. `=> validator` overrides the type's own [`Validate`] impl; validators are
//! plain `Fn(&PyObj, &mut Cx) -> Option<T>` values, see [`v`].
//!
//! Known deviations (each pinned by a golden case marked as such in the test):
//! - `int` fields are i64: values outside i64 get "Unable to parse input string as an integer, exceeded
//!   maximum size" where Python would accept the big int (Postgres would then reject it anyway).
//! - `Decimal` fields are rust_decimal (28 digits): `str()` of a value written with an exponent differs
//!   (`1E+2` is kept as `100`), and values beyond 28 digits are refused as "Input should be a valid decimal".
//! - Non-ASCII digits (`'١٢'`) are not accepted where Python's int()/Decimal() would take them.

pub mod email;
pub mod extract;
pub mod pyobj;
pub mod types;
pub mod v;

use std::fmt;

pub use extract::{body_from_bytes, BodyInput, Inputs, NoParams, PyJson, PyPath, PyQuery};
pub use pyobj::{json_loads, py_float_repr, JsonLoadError, PyInt, PyObj};
pub use types::{naive_local_to_utc, DateTimeIn, EmailStr};

/// One element of a Pydantic `loc` tuple.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loc {
    Key(String),
    Index(usize),
}

impl fmt::Display for Loc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Loc::Key(k) => f.write_str(k),
            Loc::Index(i) => write!(f, "{i}"),
        }
    }
}

/// One entry of `RequestValidationError.errors()`: the parts the app's 422 handler reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineError {
    pub loc: Vec<Loc>,
    pub msg: String,
}

impl LineError {
    /// `" -> ".join(str(l) for l in loc if l != "body")`, as backend/app/core/exceptions.py renders it.
    pub fn field(&self) -> String {
        self.loc
            .iter()
            .filter(|l| !matches!(l, Loc::Key(k) if k == "body"))
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(" -> ")
    }
}

/// Validation context: the current location and the errors collected so far.
#[derive(Default, Debug)]
pub struct Cx {
    loc: Vec<Loc>,
    pub errors: Vec<LineError>,
}

impl Cx {
    pub fn new() -> Cx {
        Cx::default()
    }

    /// A context whose locations start with `prefix` (FastAPI's "body" / "query" / "path").
    pub fn with_prefix(prefix: &str) -> Cx {
        Cx {
            loc: vec![Loc::Key(prefix.to_string())],
            errors: Vec::new(),
        }
    }

    /// Records an error at the current location.
    pub fn err(&mut self, msg: impl Into<String>) {
        self.errors.push(LineError {
            loc: self.loc.clone(),
            msg: msg.into(),
        });
    }

    /// Runs `f` one level deeper.
    pub fn at<T>(&mut self, loc: Loc, f: impl FnOnce(&mut Cx) -> T) -> T {
        self.loc.push(loc);
        let out = f(self);
        self.loc.pop();
        out
    }

    pub fn at_key<T>(&mut self, key: &str, f: impl FnOnce(&mut Cx) -> T) -> T {
        self.at(Loc::Key(key.to_string()), f)
    }

    pub fn has_errors_since(&self, mark: usize) -> bool {
        self.errors.len() > mark
    }

    pub fn mark(&self) -> usize {
        self.errors.len()
    }

    /// Drops errors recorded after `mark` (used when a fallback path replaces them).
    pub fn truncate(&mut self, mark: usize) {
        self.errors.truncate(mark);
    }
}

/// A type Pydantic knows how to validate. On failure the impl records at least one error in `cx` (at the
/// current location) and returns None.
pub trait Validate: Sized {
    fn validate(v: &PyObj, cx: &mut Cx) -> Option<Self>;
}

/// Validates a whole value and returns either the result or the collected errors.
pub fn validate_value<T: Validate>(v: &PyObj, prefix: Option<&str>) -> Result<T, Vec<LineError>> {
    let mut cx = match prefix {
        Some(p) => Cx::with_prefix(p),
        None => Cx::new(),
    };
    match T::validate(v, &mut cx) {
        Some(t) if cx.errors.is_empty() => Ok(t),
        _ => Err(cx.errors),
    }
}

/// Python field names that are Rust keywords are written `r#type`; the input key is `type`.
pub fn field_name(ident: &'static str) -> &'static str {
    ident.strip_prefix("r#").unwrap_or(ident)
}

pub const MSG_MODEL_TYPE: &str =
    "Input should be a valid dictionary or object to extract fields from";
pub const MSG_MISSING: &str = "Field required";
pub const MSG_EXTRA: &str = "Extra inputs are not permitted";

/// The model-level dict check every Pydantic model does first.
pub fn model_dict<'a>(v: &'a PyObj, cx: &mut Cx) -> Option<&'a indexmap::IndexMap<String, PyObj>> {
    match v {
        PyObj::Dict(d) => Some(d),
        _ => {
            cx.err(MSG_MODEL_TYPE);
            None
        }
    }
}

/// One model field: validates the value under its key, or applies `default` when the key is absent
/// (no default: "Field required" at the key).
pub fn model_field<T>(
    cx: &mut Cx,
    dict: &indexmap::IndexMap<String, PyObj>,
    name: &str,
    validator: impl FnOnce(&PyObj, &mut Cx) -> Option<T>,
    default: Option<Box<dyn FnOnce() -> T + '_>>,
) -> Option<T> {
    match dict.get(name) {
        Some(val) => cx.at_key(name, |cx| validator(val, cx)),
        None => match default {
            Some(d) => Some(d()),
            None => {
                cx.at_key(name, |cx| cx.err(MSG_MISSING));
                None
            }
        },
    }
}

/// `model_config = ConfigDict(extra="forbid")`: every unknown key, in input order, after field errors.
pub fn forbid_extra(cx: &mut Cx, dict: &indexmap::IndexMap<String, PyObj>, known: &[&str]) {
    for k in dict.keys() {
        if !known.contains(&k.as_str()) {
            cx.at_key(k, |cx| cx.err(MSG_EXTRA));
        }
    }
}

/// Declares a request model with Pydantic semantics. See the module docs.
#[macro_export]
macro_rules! py_model {
    (
        forbid_extra;
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $( $(#[$fmeta:meta])* $fvis:vis $field:ident : $ty:ty $(= $default:expr)? $(=> $validator:expr)? ),* $(,)?
        }
    ) => {
        $crate::py_model!(@impl true; $(#[$meta])* $vis struct $name {
            $( $(#[$fmeta])* $fvis $field : $ty $(= $default)? $(=> $validator)? ),*
        });
    };
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $( $(#[$fmeta:meta])* $fvis:vis $field:ident : $ty:ty $(= $default:expr)? $(=> $validator:expr)? ),* $(,)?
        }
    ) => {
        $crate::py_model!(@impl false; $(#[$meta])* $vis struct $name {
            $( $(#[$fmeta])* $fvis $field : $ty $(= $default)? $(=> $validator)? ),*
        });
    };
    (@impl $forbid:expr; $(#[$meta:meta])* $vis:vis struct $name:ident {
        $( $(#[$fmeta:meta])* $fvis:vis $field:ident : $ty:ty $(= $default:expr)? $(=> $validator:expr)? ),*
    }) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        $vis struct $name {
            $( $(#[$fmeta])* $fvis $field : $ty ),*
        }

        impl $crate::validation::Validate for $name {
            #[allow(unused_variables, non_snake_case, clippy::redundant_closure_call)]
            fn validate(
                v: &$crate::validation::PyObj,
                cx: &mut $crate::validation::Cx,
            ) -> ::std::option::Option<Self> {
                let dict = $crate::validation::model_dict(v, cx)?;
                let mark = cx.mark();
                $(
                    let $field: ::std::option::Option<$ty> = $crate::validation::model_field(
                        cx,
                        dict,
                        $crate::validation::field_name(stringify!($field)),
                        $crate::py_model!(@validator $ty $(, $validator)?),
                        $crate::py_model!(@default $ty $(, $default)?),
                    );
                )*
                if $forbid {
                    $crate::validation::forbid_extra(
                        cx,
                        dict,
                        &[$($crate::validation::field_name(stringify!($field))),*],
                    );
                }
                if cx.has_errors_since(mark) {
                    return ::std::option::Option::None;
                }
                ::std::option::Option::Some(Self { $( $field: $field.expect("validated") ),* })
            }
        }
    };
    (@validator $ty:ty) => {
        |v: &$crate::validation::PyObj, cx: &mut $crate::validation::Cx| {
            <$ty as $crate::validation::Validate>::validate(v, cx)
        }
    };
    (@validator $ty:ty, $validator:expr) => {
        |v: &$crate::validation::PyObj, cx: &mut $crate::validation::Cx| -> ::std::option::Option<$ty> {
            ($validator)(v, cx)
        }
    };
    (@default $ty:ty) => {
        ::std::option::Option::<::std::boxed::Box<dyn FnOnce() -> $ty>>::None
    };
    (@default $ty:ty, $default:expr) => {
        ::std::option::Option::Some(::std::boxed::Box::new(|| -> $ty { ($default).into() })
            as ::std::boxed::Box<dyn FnOnce() -> $ty>)
    };
}

/// Declares a `Literal["a", "b"]` (or a `str` Enum) as a Rust enum.
///
/// ```ignore
/// granary_core::py_literal! { pub enum Kind { A = "a", B = "b" } }
/// ```
#[macro_export]
macro_rules! py_literal {
    ($(#[$meta:meta])* $vis:vis enum $name:ident { $( $variant:ident = $value:literal ),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        $vis enum $name { $( $variant ),+ }

        impl $name {
            pub const VALUES: &'static [&'static str] = &[$($value),+];

            pub fn as_str(&self) -> &'static str {
                match self { $( $name::$variant => $value ),+ }
            }

            pub fn parse(s: &str) -> ::std::option::Option<Self> {
                match s { $( $value => ::std::option::Option::Some($name::$variant), )+ _ => ::std::option::Option::None }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> ::std::result::Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl $crate::validation::Validate for $name {
            fn validate(
                v: &$crate::validation::PyObj,
                cx: &mut $crate::validation::Cx,
            ) -> ::std::option::Option<Self> {
                if let $crate::validation::PyObj::Str(s) = v {
                    if let ::std::option::Option::Some(x) = $name::parse(s) {
                        return ::std::option::Option::Some(x);
                    }
                }
                cx.err($crate::validation::types::literal_message(Self::VALUES));
                ::std::option::Option::None
            }
        }
    };
}
