//! Python-identical primitives. Every function here reproduces what the Python backend produced, byte for
//! byte, and is golden-tested against fixtures recorded from the Python code (compat-tests/golden/).
//! Ownership during wave 0: pyjson/pyfmt (agent A), crypto/* (agent B), text/* (agent C).

/// JWT, bcrypt, Fernet, token formats, HMAC codes, Ed25519 licences.
pub mod crypto;
/// f-string number formatting, strftime with English names, html.escape.
pub mod pyfmt;
/// `json.dumps(..., sort_keys=True, default=str)` and friends, `str(Decimal)`, `str(datetime)`, isoformat.
pub mod pyjson;
/// difflib, phone normalisation, relativedelta, business days, LIKE escaping, regex helpers.
pub mod text;
