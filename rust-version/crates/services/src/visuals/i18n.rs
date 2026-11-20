//! The few words a figure prints itself (axis words, legend words). Figures are English; the `lang`
//! parameters stay so a spec's `language` keeps its one place in the code.

use super::spec::{Language, TimeUnit};

/// `word(key, language)`: a fixed figure word; unknown keys give "".
pub fn word(key: &str, _lang: Language) -> &'static str {
    match key {
        "day" => "Day",
        "week" => "Week",
        "month" => "Month",
        "days" => "days",
        "weeks" => "weeks",
        "months" => "months",
        "total" => "Total",
        "calculation" => "Calculation",
        "source" => "Source",
        "milestone" => "Milestone",
        "increase" => "Increase",
        "decrease" => "Decrease",
        "other" => "Other",
        "more" => "more",
        "ethernet" => "Ethernet",
        "poe" => "PoE",
        "fiber" => "Fibre",
        "4g" => "4G",
        "wifi" => "Wi-Fi",
        "power" => "Power",
        "data" => "Data",
        "api" => "API",
        "relay" => "Relay",
        _ => "",
    }
}

/// "Week" for a time unit (axis title).
pub fn unit_title(u: TimeUnit, lang: Language) -> &'static str {
    match u {
        TimeUnit::Day => word("day", lang),
        TimeUnit::Week => word("week", lang),
        TimeUnit::Month => word("month", lang),
    }
}

/// "week" for a time unit after the number 1.
pub fn unit_singular(u: TimeUnit, _lang: Language) -> &'static str {
    match u {
        TimeUnit::Day => "day",
        TimeUnit::Week => "week",
        TimeUnit::Month => "month",
    }
}

/// "3 weeks", "1 week", "1.5 weeks".
pub fn duration(v: f64, u: TimeUnit, lang: Language) -> String {
    let word = if (v - 1.0).abs() < 1e-9 {
        unit_singular(u, lang)
    } else {
        unit_plural(u, lang)
    };
    format!("{} {word}", number(v, 1, lang))
}

/// "weeks" for a time unit (inside sentences and axis units).
pub fn unit_plural(u: TimeUnit, lang: Language) -> &'static str {
    match u {
        TimeUnit::Day => word("days", lang),
        TimeUnit::Week => word("weeks", lang),
        TimeUnit::Month => word("months", lang),
    }
}

/// A number with grouping and decimal marks, at most `decimals` places, trailing zeros dropped:
/// 12,500.5
pub fn number(v: f64, decimals: usize, _lang: Language) -> String {
    let neg = v < 0.0;
    let s = format!("{:.*}", decimals, v.abs());
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), f.trim_end_matches('0').to_string()),
        None => (s, String::new()),
    };
    let (group, dec) = (',', '.');
    let mut out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(group);
        }
        out.push(ch);
    }
    if !frac.is_empty() {
        out.push(dec);
        out.push_str(&frac);
    }
    if neg && out.chars().any(|c| c.is_ascii_digit() && c != '0') {
        out.insert(0, '-');
    }
    out
}
