//! `urllib.robotparser.RobotFileParser` (CPython 3.12.15): `parse(lines)` and `can_fetch(useragent, url)`,
//! the two calls main.py makes. Quirks kept: the user agent is matched by its first `/`-separated token
//! as a substring, rule paths are `quote()`d (so `*` never matches as a wildcard), the first matching
//! entry and the first matching rule win, and the `*` entry is consulted last.

use granary_compat::text::pystr;

use crate::pyurl;

#[derive(Debug, Default, Clone)]
struct Entry {
    useragents: Vec<String>,
    rulelines: Vec<(String, bool)>,
}

impl Entry {
    fn applies_to(&self, useragent: &str) -> bool {
        let token = pystr::lower(useragent.split('/').next().unwrap_or(""));
        for agent in &self.useragents {
            if agent == "*" {
                return true;
            }
            if token.contains(&pystr::lower(agent)) {
                return true;
            }
        }
        false
    }

    fn allowance(&self, filename: &str) -> bool {
        for (path, allow) in &self.rulelines {
            if path == "*" || filename.starts_with(path.as_str()) {
                return *allow;
            }
        }
        true
    }
}

fn rule_line(path: &str, allowance: bool) -> (String, bool) {
    let allowance = if path.is_empty() && !allowance {
        true
    } else {
        allowance
    };
    // urlunparse(urlparse(path)); a ValueError here propagates in Python (robots.txt then counts as
    // unreadable, see main.rs).
    let normalized = match pyurl::urlparse(path) {
        Ok(p) => pyurl::urlunparse(&p),
        Err(_) => path.to_string(),
    };
    (pyurl::quote(&normalized, "/"), allowance)
}

#[derive(Debug, Default, Clone)]
pub struct RobotFileParser {
    entries: Vec<Entry>,
    default_entry: Option<Entry>,
}

impl RobotFileParser {
    fn add_entry(&mut self, entry: Entry) {
        if entry.useragents.iter().any(|a| a == "*") {
            if self.default_entry.is_none() {
                self.default_entry = Some(entry);
            }
        } else {
            self.entries.push(entry);
        }
    }

    /// `parse(lines)`. `Err` where Python raises (a ValueError from `urlparse` in a rule path).
    pub fn parse(lines: &[&str]) -> Result<Self, String> {
        let mut parser = RobotFileParser::default();
        let mut state = 0;
        let mut entry = Entry::default();
        for raw in lines {
            if raw.is_empty() {
                if state == 1 {
                    entry = Entry::default();
                    state = 0;
                } else if state == 2 {
                    parser.add_entry(std::mem::take(&mut entry));
                    state = 0;
                }
            }
            let line = match raw.find('#') {
                Some(i) => &raw[..i],
                None => raw,
            };
            let line = pystr::strip(line);
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = pystr::lower(pystr::strip(key));
            let value = pyurl::unquote(pystr::strip(value));
            match key.as_str() {
                "user-agent" => {
                    if state == 2 {
                        parser.add_entry(std::mem::take(&mut entry));
                    }
                    entry.useragents.push(value);
                    state = 1;
                }
                "disallow" | "allow" if state != 0 => {
                    if pyurl::urlparse(&value).is_err() {
                        return Err("ValueError in robots.txt rule".into());
                    }
                    entry.rulelines.push(rule_line(&value, key == "allow"));
                    state = 2;
                }
                "crawl-delay" | "request-rate" if state != 0 => {
                    // isdigit() passes superscript digits that int() then refuses: a ValueError.
                    let parts: Vec<&str> = if key == "crawl-delay" {
                        vec![value.as_str()]
                    } else {
                        value.split('/').collect()
                    };
                    let all_digits = |s: &str| {
                        let s = pystr::strip(s);
                        !s.is_empty() && s.chars().all(pystr::is_digit)
                    };
                    let sane = if key == "crawl-delay" {
                        all_digits(parts[0])
                    } else {
                        parts.len() == 2 && parts.iter().all(|p| all_digits(p))
                    };
                    if sane
                        && !parts
                            .iter()
                            .all(|p| pystr::strip(p).chars().all(pystr::is_decimal))
                    {
                        return Err("ValueError in robots.txt delay".into());
                    }
                    state = 2;
                }
                _ => {}
            }
        }
        if state == 2 {
            parser.add_entry(entry);
        }
        Ok(parser)
    }

    /// `can_fetch(useragent, url)` after a parse (last_checked is set).
    pub fn can_fetch(&self, useragent: &str, url: &str) -> Result<bool, String> {
        let parsed = pyurl::urlparse(&pyurl::unquote(url))?;
        let rebuilt = pyurl::urlunparse(&pyurl::Parse {
            scheme: String::new(),
            netloc: String::new(),
            path: parsed.path,
            params: parsed.params,
            query: parsed.query,
            fragment: parsed.fragment,
        });
        let mut url = pyurl::quote(&rebuilt, "/");
        if url.is_empty() {
            url = "/".into();
        }
        for entry in &self.entries {
            if entry.applies_to(useragent) {
                return Ok(entry.allowance(&url));
            }
        }
        if let Some(d) = &self.default_entry {
            return Ok(d.allowance(&url));
        }
        Ok(true)
    }
}
