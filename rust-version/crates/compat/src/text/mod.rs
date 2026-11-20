//! Owned by agent C (wave 0). Python text semantics the backend relies on, golden-tested against
//! fixtures recorded from the real backend functions (compat-tests/golden/gen_text.py,
//! crates/compat/tests/text_golden.rs).

/// difflib.SequenceMatcher(None, a, b): ratio / quick_ratio / real_quick_ratio, edit share in %.
pub mod difflib;
/// Name normalisation and similarity for matching and dedupe (crm/dedupe, nary/identity_match, research).
pub mod matching;
/// The backend's lookaround regexes with Python `re` semantics (openai, embeddings, erp_guide, persona).
pub mod patterns;
/// Python phonenumbers 8.13.51 (parse / is_valid_number / region / format) on Python's own metadata.
pub mod phonelib;
/// nary/phones.py.
pub mod phones;
/// Python `str` methods and character classes (isspace, isdigit, `\w`, lower, title, split, strip, ...).
pub mod pystr;
/// dateutil.relativedelta(years=, months=, days=) with month-end clamping.
pub mod relativedelta;
/// Generated Python 3.12 / Unicode 15.0 character tables and the pytz zone -> country map.
mod unicode_tables;
/// core/utils.py: normalize_phone, add_business_days, escape_like, parse_uuid, get_client_ip.
pub mod utils;
