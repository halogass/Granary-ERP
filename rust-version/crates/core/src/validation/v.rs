//! Validators with constraints: `Field(min_length=..)`, `conint(ge=..)`, `condecimal(..)`, `pattern=..`,
//! list length. Bounds are written as the Python literal (`"0"`, `"0.01"`) because Pydantic prints them
//! with `str()` in the message.

use std::collections::HashMap;
use std::sync::Mutex;

use rust_decimal::Decimal;

use super::pyobj::PyObj;
use super::types::{self, DecimalChecks, MSG_LIST_TYPE, MSG_STRING_TYPE};
use super::{Cx, Loc, Validate};

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// String constraints, checked in pydantic-core's order: min_length, max_length, pattern.
#[derive(Clone, Debug, Default)]
pub struct StrC {
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub pattern: Option<&'static str>,
}

impl StrC {
    pub fn new() -> StrC {
        StrC::default()
    }
    pub fn min_length(mut self, n: usize) -> StrC {
        self.min_length = Some(n);
        self
    }
    pub fn max_length(mut self, n: usize) -> StrC {
        self.max_length = Some(n);
        self
    }
    pub fn pattern(mut self, p: &'static str) -> StrC {
        self.pattern = Some(p);
        self
    }
}

static PATTERNS: once_cell::sync::Lazy<Mutex<HashMap<&'static str, regex::Regex>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

fn pattern_matches(p: &'static str, s: &str) -> bool {
    let mut cache = PATTERNS.lock().expect("pattern cache");
    let re = cache
        .entry(p)
        .or_insert_with(|| regex::Regex::new(p).expect("valid pattern"));
    re.is_match(s)
}

pub fn check_str(s: String, cx: &mut Cx, c: &StrC) -> Option<String> {
    let n = s.chars().count();
    if let Some(min) = c.min_length {
        if n < min {
            cx.err(format!(
                "String should have at least {min} character{}",
                plural(min)
            ));
            return None;
        }
    }
    if let Some(max) = c.max_length {
        if n > max {
            cx.err(format!(
                "String should have at most {max} character{}",
                plural(max)
            ));
            return None;
        }
    }
    if let Some(p) = c.pattern {
        if !pattern_matches(p, &s) {
            cx.err(format!("String should match pattern '{p}'"));
            return None;
        }
    }
    Some(s)
}

/// `str` with constraints.
pub fn str_c(c: StrC) -> impl Fn(&PyObj, &mut Cx) -> Option<String> {
    move |v, cx| match v {
        PyObj::Str(s) => check_str(s.clone(), cx, &c),
        _ => {
            cx.err(MSG_STRING_TYPE);
            None
        }
    }
}

/// Numeric bounds as Python literals.
#[derive(Clone, Debug, Default)]
pub struct NumC {
    pub ge: Option<&'static str>,
    pub gt: Option<&'static str>,
    pub le: Option<&'static str>,
    pub lt: Option<&'static str>,
    pub multiple_of: Option<&'static str>,
}

impl NumC {
    pub fn new() -> NumC {
        NumC::default()
    }
    pub fn ge(mut self, b: &'static str) -> NumC {
        self.ge = Some(b);
        self
    }
    pub fn gt(mut self, b: &'static str) -> NumC {
        self.gt = Some(b);
        self
    }
    pub fn le(mut self, b: &'static str) -> NumC {
        self.le = Some(b);
        self
    }
    pub fn lt(mut self, b: &'static str) -> NumC {
        self.lt = Some(b);
        self
    }
    pub fn multiple_of(mut self, b: &'static str) -> NumC {
        self.multiple_of = Some(b);
        self
    }
}

/// Order of pydantic-core's ConstrainedInt/FloatValidator: multiple_of, le, lt, ge, gt.
/// Negated comparisons on purpose: NaN fails every bound, as in Python.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn check_bounds<T: PartialOrd + Copy>(
    x: T,
    cx: &mut Cx,
    c: &NumC,
    parse: fn(&str) -> T,
    is_multiple: impl Fn(T, T) -> bool,
) -> Option<T> {
    if let Some(m) = c.multiple_of {
        if !is_multiple(x, parse(m)) {
            cx.err(format!("Input should be a multiple of {m}"));
            return None;
        }
    }
    if let Some(b) = c.le {
        if !(x <= parse(b)) {
            cx.err(format!("Input should be less than or equal to {b}"));
            return None;
        }
    }
    if let Some(b) = c.lt {
        if !(x < parse(b)) {
            cx.err(format!("Input should be less than {b}"));
            return None;
        }
    }
    if let Some(b) = c.ge {
        if !(x >= parse(b)) {
            cx.err(format!("Input should be greater than or equal to {b}"));
            return None;
        }
    }
    if let Some(b) = c.gt {
        if !(x > parse(b)) {
            cx.err(format!("Input should be greater than {b}"));
            return None;
        }
    }
    Some(x)
}

/// `conint(...)` / `int = Field(ge=..)`.
pub fn int_c(c: NumC) -> impl Fn(&PyObj, &mut Cx) -> Option<i64> {
    move |v, cx| {
        let x = types::validate_int(v, cx)?;
        check_bounds(
            x,
            cx,
            &c,
            |s| s.parse::<f64>().expect("int bound") as i64,
            |a, m| m != 0 && a % m == 0,
        )
    }
}

/// `confloat(...)` / `float = Field(ge=..)`.
pub fn float_c(c: NumC) -> impl Fn(&PyObj, &mut Cx) -> Option<f64> {
    move |v, cx| {
        let x = types::validate_float(v, cx)?;
        check_bounds(
            x,
            cx,
            &c,
            |s| s.parse::<f64>().expect("float bound"),
            |a, m| (a / m).fract() == 0.0,
        )
    }
}

/// Decimal constraints.
#[derive(Clone, Debug, Default)]
pub struct DecC {
    pub num: NumC,
    pub max_digits: Option<u64>,
    pub decimal_places: Option<u64>,
}

impl DecC {
    pub fn new() -> DecC {
        DecC::default()
    }
    pub fn ge(mut self, b: &'static str) -> DecC {
        self.num.ge = Some(b);
        self
    }
    pub fn gt(mut self, b: &'static str) -> DecC {
        self.num.gt = Some(b);
        self
    }
    pub fn le(mut self, b: &'static str) -> DecC {
        self.num.le = Some(b);
        self
    }
    pub fn lt(mut self, b: &'static str) -> DecC {
        self.num.lt = Some(b);
        self
    }
    pub fn max_digits(mut self, n: u64) -> DecC {
        self.max_digits = Some(n);
        self
    }
    pub fn decimal_places(mut self, n: u64) -> DecC {
        self.decimal_places = Some(n);
        self
    }
}

/// `condecimal(...)`.
pub fn decimal_c(c: DecC) -> impl Fn(&PyObj, &mut Cx) -> Option<Decimal> {
    let checks = DecimalChecks {
        max_digits: c.max_digits,
        decimal_places: c.decimal_places,
        le: c.num.le.map(str::to_string),
        lt: c.num.lt.map(str::to_string),
        ge: c.num.ge.map(str::to_string),
        gt: c.num.gt.map(str::to_string),
        multiple_of: c.num.multiple_of.map(str::to_string),
    };
    move |v, cx| types::validate_decimal_with(v, cx, &checks)
}

/// `Optional[...]` around a constrained validator: None passes untouched.
pub fn nullable<T>(
    inner: impl Fn(&PyObj, &mut Cx) -> Option<T>,
) -> impl Fn(&PyObj, &mut Cx) -> Option<Option<T>> {
    move |v, cx| match v {
        PyObj::None => Some(None),
        _ => inner(v, cx).map(Some),
    }
}

/// A Python field typed `T` with a `None` default (`x: str = None`): absent gives None, but an explicit
/// null is validated as `T` (and refused).
pub fn not_null<T: Validate>() -> impl Fn(&PyObj, &mut Cx) -> Option<Option<T>> {
    move |v, cx| T::validate(v, cx).map(Some)
}

/// List constraints.
#[derive(Clone, Debug, Default)]
pub struct ListC {
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
}

impl ListC {
    pub fn new() -> ListC {
        ListC::default()
    }
    pub fn min_length(mut self, n: usize) -> ListC {
        self.min_length = Some(n);
        self
    }
    pub fn max_length(mut self, n: usize) -> ListC {
        self.max_length = Some(n);
        self
    }
}

/// pydantic-core `validate_iter_to_vec` + list length checks: an item over max_length (counted
/// whether it validated or not) replaces every error with "List should have at most ...".
pub fn list_with<T>(
    v: &PyObj,
    cx: &mut Cx,
    c: &ListC,
    item: impl Fn(&PyObj, &mut Cx) -> Option<T>,
) -> Option<Vec<T>> {
    let PyObj::List(items) = v else {
        cx.err(MSG_LIST_TYPE);
        return None;
    };
    let mark = cx.mark();
    let mut out = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        let r = cx.at(Loc::Index(i), |cx| item(it, cx));
        if let Some(max) = c.max_length {
            if i + 1 > max {
                cx.truncate(mark);
                cx.err(format!(
                    "List should have at most {max} item{} after validation, not {}",
                    plural(max),
                    items.len()
                ));
                return None;
            }
        }
        if let Some(x) = r {
            out.push(x);
        }
    }
    if cx.has_errors_since(mark) {
        return None;
    }
    if let Some(min) = c.min_length {
        if out.len() < min {
            cx.err(format!(
                "List should have at least {min} item{} after validation, not {}",
                plural(min),
                out.len()
            ));
            return None;
        }
    }
    Some(out)
}

/// `list[T] = Field(min_length=.., max_length=..)` with T's own validator.
pub fn list_c<T: Validate>(c: ListC) -> impl Fn(&PyObj, &mut Cx) -> Option<Vec<T>> {
    move |v, cx| list_with(v, cx, &c, T::validate)
}

/// `list[...]` whose items need a custom validator (e.g. `list[constr(...)]`).
pub fn list_of<T>(
    c: ListC,
    item: impl Fn(&PyObj, &mut Cx) -> Option<T>,
) -> impl Fn(&PyObj, &mut Cx) -> Option<Vec<T>> {
    move |v, cx| list_with(v, cx, &c, &item)
}
