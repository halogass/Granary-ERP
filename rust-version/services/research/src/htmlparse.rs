//! `BeautifulSoup(html, "html.parser")` as clean.py uses it: CPython 3.12.15 `html.parser.HTMLParser`
//! (convert_charrefs=False, as bs4 runs it) feeding Beautiful Soup 4.12.3's tree builder
//! (`BeautifulSoupHTMLParser` + `BeautifulSoup.handle_*`, void elements, string containers). Only the
//! tree shape and the string classes clean.py looks at are kept.

use fancy_regex::Regex as FancyRegex;
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use regex::Regex;

use granary_compat::text::pystr;

use crate::entities_data as ent;

/// The NavigableString subclass a string was created as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrClass {
    Navigable,
    CData,
    Comment,
    Doctype,
    Declaration,
    ProcessingInstruction,
    RubyText,
    RubyParenthesis,
    Stylesheet,
    Script,
    Template,
}

#[derive(Debug)]
pub enum NodeKind {
    Tag {
        name: String,
        attrs: IndexMap<String, String>,
    },
    Text {
        text: String,
        class: StrClass,
    },
}

#[derive(Debug)]
pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

/// The parsed document; node 0 is the `[document]` root.
#[derive(Debug)]
pub struct Soup {
    pub nodes: Vec<Node>,
}

const ROOT: &str = "[document]";
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "keygen", "link", "menuitem",
    "meta", "param", "source", "track", "wbr", "basefont", "bgsound", "command", "frame", "image",
    "isindex", "nextid", "spacer",
];

fn container_class(tag: &str) -> Option<StrClass> {
    match tag {
        "rt" => Some(StrClass::RubyText),
        "rp" => Some(StrClass::RubyParenthesis),
        "style" => Some(StrClass::Stylesheet),
        "script" => Some(StrClass::Script),
        "template" => Some(StrClass::Template),
        _ => None,
    }
}

impl Soup {
    pub fn name(&self, i: usize) -> Option<&str> {
        match &self.nodes[i].kind {
            NodeKind::Tag { name, .. } => Some(name),
            NodeKind::Text { .. } => None,
        }
    }

    pub fn attr(&self, i: usize, key: &str) -> Option<&str> {
        match &self.nodes[i].kind {
            NodeKind::Tag { attrs, .. } => attrs.get(key).map(String::as_str),
            NodeKind::Text { .. } => None,
        }
    }

    /// `.descendants`: document order, the node itself excluded.
    pub fn descendants(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack: Vec<usize> = self.nodes[i].children.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            out.push(n);
            stack.extend(self.nodes[n].children.iter().rev().copied());
        }
        out
    }

    /// `find_all(names)` on the whole document.
    pub fn find_all(&self, names: &[&str]) -> Vec<usize> {
        self.descendants(0)
            .into_iter()
            .filter(|&n| self.name(n).is_some_and(|nm| names.contains(&nm)))
            .collect()
    }

    /// `decompose()`: the node and everything under it leave the tree.
    pub fn decompose(&mut self, i: usize) {
        if let Some(p) = self.nodes[i].parent.take() {
            self.nodes[p].children.retain(|&c| c != i);
        }
    }

    /// `el.find_parent(names)` is not None.
    pub fn has_ancestor(&self, i: usize, names: &[&str]) -> bool {
        let mut cur = self.nodes[i].parent;
        while let Some(p) = cur {
            if self.name(p).is_some_and(|nm| names.contains(&nm)) {
                return true;
            }
            cur = self.nodes[p].parent;
        }
        false
    }

    /// `tag.get_text(separator, strip=True)` with the default string types (NavigableString and CData
    /// for an ordinary tag; the container class for rt/rp/style/script/template).
    pub fn get_text_stripped(&self, i: usize, separator: &str) -> String {
        let wanted: Vec<StrClass> = match self.name(i).and_then(container_class) {
            Some(c) => vec![c],
            None => vec![StrClass::Navigable, StrClass::CData],
        };
        let mut parts: Vec<&str> = Vec::new();
        for n in self.descendants(i) {
            if let NodeKind::Text { text, class } = &self.nodes[n].kind {
                if wanted.contains(class) {
                    let s = pystr::strip(text);
                    if !s.is_empty() {
                        parts.push(s);
                    }
                }
            }
        }
        parts.join(separator)
    }
}

// ---------------------------------------------------------------------------------------------------
// Tree building (bs4 BeautifulSoup + BeautifulSoupHTMLParser)

struct Builder {
    nodes: Vec<Node>,
    tag_stack: Vec<usize>,
    open_tag_counter: std::collections::HashMap<String, usize>,
    string_container_stack: Vec<usize>,
    current_data: Vec<String>,
    already_closed_empty_element: Vec<String>,
}

impl Builder {
    fn new() -> Self {
        let root = Node {
            kind: NodeKind::Tag {
                name: ROOT.into(),
                attrs: IndexMap::new(),
            },
            parent: None,
            children: Vec::new(),
        };
        Builder {
            nodes: vec![root],
            tag_stack: vec![0],
            open_tag_counter: Default::default(),
            string_container_stack: Vec::new(),
            current_data: Vec::new(),
            already_closed_empty_element: Vec::new(),
        }
    }

    fn current(&self) -> usize {
        *self.tag_stack.last().unwrap_or(&0)
    }

    fn tag_name(&self, i: usize) -> &str {
        match &self.nodes[i].kind {
            NodeKind::Tag { name, .. } => name,
            NodeKind::Text { .. } => "",
        }
    }

    fn end_data(&mut self, class: Option<StrClass>) {
        if self.current_data.is_empty() {
            return;
        }
        let mut data = self.current_data.concat();
        self.current_data.clear();
        // Whitespace-only strings collapse to one space or newline (preserve-whitespace tags aside,
        // which only matters for strings clean.py strips away anyway).
        if data
            .chars()
            .all(|c| matches!(c, ' ' | '\n' | '\t' | '\x0c' | '\r'))
        {
            data = if data.contains('\n') {
                "\n".into()
            } else {
                " ".into()
            };
        }
        let class = match class {
            Some(c) => c,
            None => self
                .string_container_stack
                .last()
                .and_then(|&t| container_class(self.tag_name(t)))
                .unwrap_or(StrClass::Navigable),
        };
        let parent = self.current();
        let idx = self.nodes.len();
        self.nodes.push(Node {
            kind: NodeKind::Text { text: data, class },
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent].children.push(idx);
    }

    fn pop_tag(&mut self) {
        let Some(tag) = self.tag_stack.pop() else {
            return;
        };
        let name = self.tag_name(tag).to_string();
        if let Some(c) = self.open_tag_counter.get_mut(&name) {
            *c = c.saturating_sub(1);
        }
        if self.string_container_stack.last() == Some(&tag) {
            self.string_container_stack.pop();
        }
    }

    fn pop_to_tag(&mut self, name: &str) {
        if name == ROOT {
            return;
        }
        let size = self.tag_stack.len();
        for i in (1..size).rev() {
            if self.open_tag_counter.get(name).copied().unwrap_or(0) == 0 {
                break;
            }
            let t = self.tag_stack[i];
            if self.tag_name(t) == name {
                self.pop_tag();
                break;
            }
            self.pop_tag();
        }
    }

    fn soup_starttag(&mut self, name: &str, attrs: IndexMap<String, String>) -> usize {
        self.end_data(None);
        let parent = self.current();
        let idx = self.nodes.len();
        self.nodes.push(Node {
            kind: NodeKind::Tag {
                name: name.to_string(),
                attrs,
            },
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent].children.push(idx);
        self.tag_stack.push(idx);
        if name != ROOT {
            *self.open_tag_counter.entry(name.to_string()).or_insert(0) += 1;
        }
        if container_class(name).is_some() {
            self.string_container_stack.push(idx);
        }
        idx
    }

    fn soup_endtag(&mut self, name: &str) {
        self.end_data(None);
        self.pop_to_tag(name);
    }

    // BeautifulSoupHTMLParser handlers

    fn handle_starttag(
        &mut self,
        name: &str,
        attrs: Vec<(String, Option<String>)>,
        handle_empty_element: bool,
    ) {
        let mut dict: IndexMap<String, String> = IndexMap::new();
        for (k, v) in attrs {
            dict.insert(k, v.unwrap_or_default());
        }
        let tag = self.soup_starttag(name, dict);
        let is_empty = self.nodes[tag].children.is_empty() && VOID.contains(&name);
        if is_empty && handle_empty_element {
            self.handle_endtag(name, false);
            self.already_closed_empty_element.push(name.to_string());
        }
    }

    fn handle_endtag(&mut self, name: &str, check_already_closed: bool) {
        if check_already_closed {
            if let Some(pos) = self
                .already_closed_empty_element
                .iter()
                .position(|n| n == name)
            {
                self.already_closed_empty_element.remove(pos);
                return;
            }
        }
        self.soup_endtag(name);
    }

    fn handle_startendtag(&mut self, name: &str, attrs: Vec<(String, Option<String>)>) {
        self.handle_starttag(name, attrs, false);
        self.handle_endtag(name, true);
    }

    fn handle_data(&mut self, data: &str) {
        self.current_data.push(data.to_string());
    }

    fn handle_charref(&mut self, name: &str) {
        let (digits, radix) = if let Some(h) = name.strip_prefix(['x', 'X']) {
            (h, 16)
        } else {
            (name, 10)
        };
        let mut v: u128 = 0;
        for c in digits.chars() {
            v = v
                .saturating_mul(radix as u128)
                .saturating_add(c.to_digit(radix).unwrap_or(0) as u128);
        }
        let mut data: Option<String> = None;
        if v < 256 {
            data = cp1252(v as u8).map(String::from);
        }
        if data.is_none() {
            // chr(): a lone surrogate has no Rust form (see the report); out of range gives U+FFFD.
            data = Some(
                char::from_u32(v.min(u32::MAX as u128) as u32)
                    .unwrap_or('\u{FFFD}')
                    .to_string(),
            );
        }
        self.handle_data(&data.unwrap_or_else(|| "\u{FFFD}".into()));
    }

    fn handle_entityref(&mut self, name: &str) {
        match lookup(ent::BS4_ENTITY, name) {
            Some(c) => self.handle_data(c),
            None => self.handle_data(&format!("&{name}")),
        }
    }

    fn handle_special(&mut self, data: &str, class: StrClass) {
        self.end_data(None);
        self.handle_data(data);
        self.end_data(Some(class));
    }

    fn handle_comment(&mut self, data: &str) {
        self.handle_special(data, StrClass::Comment);
    }

    fn handle_decl(&mut self, data: &str) {
        let rest: String = data.chars().skip("DOCTYPE ".len()).collect();
        self.handle_special(&rest, StrClass::Doctype);
    }

    fn unknown_decl(&mut self, data: &str) {
        let upper_starts = data.chars().take(6).collect::<String>().to_uppercase() == "CDATA[";
        if upper_starts {
            let rest: String = data.chars().skip(6).collect();
            self.handle_special(&rest, StrClass::CData);
        } else {
            self.handle_special(data, StrClass::Declaration);
        }
    }

    fn handle_pi(&mut self, data: &str) {
        self.handle_special(data, StrClass::ProcessingInstruction);
    }
}

/// `bytearray([n]).decode('windows-1252')`: `None` for the five undefined bytes.
fn cp1252(b: u8) -> Option<char> {
    const HIGH: [u16; 32] = [
        0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
        0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
        0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
    ];
    if (0x80..0xA0).contains(&b) {
        let c = HIGH[(b - 0x80) as usize];
        if c == 0 {
            None
        } else {
            char::from_u32(c as u32)
        }
    } else {
        Some(b as char)
    }
}

fn lookup<'a>(table: &'a [(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    table
        .binary_search_by(|(k, _)| (*k).cmp(key))
        .ok()
        .map(|i| table[i].1)
}

// ---------------------------------------------------------------------------------------------------
// html.unescape

static UNESCAPE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\x0c <&#;]{1,32};?)").expect("regex")
});

/// `html.unescape(s)`.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    UNESCAPE_RE
        .replace_all(s, |caps: &regex::Captures| {
            let g = &caps[1];
            if let Some(num) = g.strip_prefix('#') {
                let (digits, radix) = if let Some(h) = num.strip_prefix(['x', 'X']) {
                    (h, 16)
                } else {
                    (num, 10)
                };
                let digits = digits.trim_end_matches(';');
                let mut v: u128 = 0;
                for c in digits.chars() {
                    v = v
                        .saturating_mul(radix as u128)
                        .saturating_add(c.to_digit(radix).unwrap_or(0) as u128);
                }
                if v <= u32::MAX as u128 {
                    if let Ok(i) =
                        ent::INVALID_CHARREFS.binary_search_by_key(&(v as u32), |(k, _)| *k)
                    {
                        return ent::INVALID_CHARREFS[i].1.to_string();
                    }
                }
                if (0xD800..=0xDFFF).contains(&v) || v > 0x10FFFF {
                    return "\u{FFFD}".into();
                }
                if ent::INVALID_CODEPOINTS.binary_search(&(v as u32)).is_ok() {
                    return String::new();
                }
                return char::from_u32(v as u32)
                    .map(String::from)
                    .unwrap_or_else(|| "\u{FFFD}".into());
            }
            if let Some(v) = lookup(ent::HTML5, g) {
                return v.to_string();
            }
            // the longest matching name, as the standard defines it
            let idx: Vec<usize> = g.char_indices().map(|(i, _)| i).collect();
            for x in (2..idx.len()).rev() {
                let (head, tail) = g.split_at(idx[x]);
                if let Some(v) = lookup(ent::HTML5, head) {
                    return format!("{v}{tail}");
                }
            }
            format!("&{g}")
        })
        .into_owned()
}

// ---------------------------------------------------------------------------------------------------
// html.parser.HTMLParser

const CDATA_CONTENT_ELEMENTS: &[&str] =
    &["script", "style", "xmp", "iframe", "noembed", "noframes"];
const RCDATA_CONTENT_ELEMENTS: &[&str] = &["textarea", "title"];

static CHARREF: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^&#(?:[0-9]+|[xX][0-9a-fA-F]+)[^0-9a-fA-F]").expect("regex"));
static ENTITYREF: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^&([a-zA-Z][-.a-zA-Z0-9]*)[^a-zA-Z0-9]").expect("regex"));
static COMMENTCLOSE: Lazy<Regex> = Lazy::new(|| Regex::new(r"--!?>").expect("regex"));
static COMMENTABRUPTCLOSE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^-?>").expect("regex"));
static LOCATETAGEND: Lazy<FancyRegex> = Lazy::new(|| {
    FancyRegex::new(concat!(
        r#"^[a-zA-Z][^\t\n\r\x0c />]*"#,
        r#"[\t\n\r\x0c /]*"#,
        r#"(?:(?<=['"\t\n\r\x0c /])[^\t\n\r\x0c />][^\t\n\r\x0c /=>]*"#,
        r#"(?:[\t\n\r\x0c ]*=[\t\n\r\x0c ]*"#,
        r#"(?:'[^']*'|"[^"]*"|(?!['"])[^>\t\n\r\x0c ]*)"#,
        r#")?"#,
        r#"[\t\n\r\x0c /]*"#,
        r#")*"#,
        r#">?"#,
    ))
    .expect("regex")
});
/// attrfind_tolerant, run on the text from one character before the match position so that its
/// look-behind sees that character (`^[\s\S]` eats it).
static ATTRFIND: Lazy<FancyRegex> = Lazy::new(|| {
    FancyRegex::new(concat!(
        r#"^[\s\S]"#,
        r#"((?<=['"\t\n\r\x0c /])[^\t\n\r\x0c />][^\t\n\r\x0c /=>]*)"#,
        r#"([\t\n\r\x0c ]*=[\t\n\r\x0c ]*"#,
        r#"('[^']*'|"[^"]*"|(?!['"])[^>\t\n\r\x0c ]*)"#,
        r#")?"#,
        r#"(?:[\t\n\r\x0c ]|/(?!>))*"#,
    ))
    .expect("regex")
});

fn is_ascii_letter_at(s: &str, i: usize) -> bool {
    s.as_bytes().get(i).is_some_and(|b| b.is_ascii_alphabetic())
}

/// tagfind_tolerant.match(s, i): (tag name, end).
fn tagfind(s: &str, i: usize) -> (String, usize) {
    let b = s.as_bytes();
    let mut j = i + 1; // [a-zA-Z]
    while j < b.len() && !matches!(b[j], b'\t' | b'\n' | b'\r' | b'\x0c' | b' ' | b'/' | b'>') {
        j += 1;
    }
    let name = s[i..j].to_string();
    loop {
        match b.get(j) {
            Some(b'\t' | b'\n' | b'\r' | b'\x0c' | b' ') => j += 1,
            Some(b'/') if b.get(j + 1) != Some(&b'>') => j += 1,
            _ => break,
        }
    }
    (name, j)
}

fn lower9(s: &str, i: usize) -> String {
    s[i..]
        .chars()
        .take(9)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn prev_char_start(s: &str, k: usize) -> usize {
    k - s[..k].chars().next_back().map(char::len_utf8).unwrap_or(0)
}

struct HtmlParser {
    b: Builder,
    rawdata: String,
    cdata_elem: Option<String>,
    escapable: bool,
}

impl HtmlParser {
    fn set_cdata_mode(&mut self, elem: &str, escapable: bool) {
        self.cdata_elem = Some(pystr::lower(elem));
        self.escapable = escapable;
    }

    fn clear_cdata_mode(&mut self) {
        self.cdata_elem = None;
        self.escapable = true;
    }

    /// `self.interesting.search(rawdata, i)`.
    fn interesting(&self, raw: &str, i: usize) -> Option<usize> {
        match &self.cdata_elem {
            None => raw[i..].find(['&', '<']).map(|p| p + i),
            Some(elem) => {
                let end_tag = format!("</{elem}");
                let b = raw.as_bytes();
                let mut p = i;
                while p < b.len() {
                    if self.escapable && b[p] == b'&' {
                        return Some(p);
                    }
                    if b[p] == b'<'
                        && b.len() >= p + end_tag.len()
                        && b[p..p + end_tag.len()].eq_ignore_ascii_case(end_tag.as_bytes())
                        && b.get(p + end_tag.len()).is_some_and(|c| {
                            matches!(c, b'\t' | b'\n' | b'\r' | b'\x0c' | b' ' | b'/' | b'>')
                        })
                    {
                        return Some(p);
                    }
                    p += 1;
                }
                None
            }
        }
    }

    fn goahead(&mut self, end: bool) {
        let raw = std::mem::take(&mut self.rawdata);
        let n = raw.len();
        let mut i = 0usize;
        while i < n {
            let j = match self.interesting(&raw, i) {
                Some(j) => j,
                None => {
                    if self.cdata_elem.is_some() {
                        break;
                    }
                    n
                }
            };
            if i < j {
                self.b.handle_data(&raw[i..j]);
            }
            i = j;
            if i == n {
                break;
            }
            let rest = &raw[i..];
            if rest.starts_with('<') {
                let k: isize = if is_ascii_letter_at(&raw, i + 1) {
                    self.parse_starttag(&raw, i)
                } else if rest.starts_with("</") {
                    self.parse_endtag(&raw, i)
                } else if rest.starts_with("<!--") {
                    self.parse_comment(&raw, i)
                } else if rest.starts_with("<?") {
                    self.parse_pi(&raw, i)
                } else if rest.starts_with("<!") {
                    self.parse_html_declaration(&raw, i)
                } else if i + 1 < n || end {
                    self.b.handle_data("<");
                    (i + 1) as isize
                } else {
                    break;
                };
                if k < 0 {
                    if !end {
                        break;
                    }
                    if is_ascii_letter_at(&raw, i + 1) {
                        // an unterminated start tag swallows the rest
                    } else if rest.starts_with("</") {
                        if i + 2 == n {
                            self.b.handle_data("</");
                        } else if is_ascii_letter_at(&raw, i + 2) {
                        } else {
                            self.b.handle_comment(&raw[i + 2..]);
                        }
                    } else if rest.starts_with("<!--") {
                        let mut j = n;
                        let tail = &raw[i + 4..];
                        for suffix in ["--!", "--", "-"] {
                            if tail.ends_with(suffix) {
                                j -= suffix.len();
                                break;
                            }
                        }
                        self.b.handle_comment(&raw[i + 4..j]);
                    } else if rest.starts_with("<![CDATA[") {
                        self.b.unknown_decl(&raw[i + 3..]);
                    } else if lower9(&raw, i) == "<!doctype" {
                        self.b.handle_decl(&raw[i + 2..]);
                    } else if rest.starts_with("<!") {
                        self.b.handle_comment(&raw[i + 2..]);
                    } else if rest.starts_with("<?") {
                        self.b.handle_pi(&raw[i + 2..]);
                    }
                    i = n;
                } else {
                    i = k as usize;
                }
            } else if rest.starts_with("&#") {
                if let Some(m) = CHARREF.find(rest) {
                    let text = m.as_str();
                    let last = text.chars().next_back().unwrap_or(';');
                    let name = &text[2..text.len() - last.len_utf8()];
                    self.b.handle_charref(name);
                    let mut k = i + m.end();
                    if last != ';' {
                        k -= last.len_utf8();
                    }
                    i = k;
                    continue;
                } else {
                    if rest.contains(';') {
                        self.b.handle_data(&raw[i..i + 2]);
                        i += 2;
                    }
                    break;
                }
            } else if rest.starts_with('&') {
                if let Some(c) = ENTITYREF.captures(rest) {
                    let whole = c.get(0).map(|m| m.as_str()).unwrap_or("");
                    let last = whole.chars().next_back().unwrap_or(';');
                    self.b.handle_entityref(&c[1]);
                    let mut k = i + whole.len();
                    if last != ';' {
                        k -= last.len_utf8();
                    }
                    i = k;
                    continue;
                }
                // incomplete: '&[a-zA-Z#]'
                let b1 = rest.as_bytes().get(1).copied();
                if b1.is_some_and(|b| b.is_ascii_alphabetic() || b == b'#') {
                    if end && rest.len() == 2 {
                        i += 1;
                    }
                    break;
                } else if i + 1 < n {
                    self.b.handle_data("&");
                    i += 1;
                } else {
                    break;
                }
            } else {
                break; // interesting.search() lied
            }
        }
        if end && i < n {
            self.b.handle_data(&raw[i..n]);
            i = n;
        }
        self.rawdata = raw[i..].to_string();
    }

    fn parse_html_declaration(&mut self, raw: &str, i: usize) -> isize {
        let rest = &raw[i..];
        if rest.starts_with("<!--") {
            return self.parse_comment(raw, i);
        }
        if rest.starts_with("<![CDATA[") {
            return match raw[i + 9..].find("]]>") {
                None => -1,
                Some(p) => {
                    let j = i + 9 + p;
                    self.b.unknown_decl(&raw[i + 3..j]);
                    (j + 3) as isize
                }
            };
        }
        if lower9(raw, i) == "<!doctype" {
            let start = (i + 9).min(raw.len());
            let start = if raw.is_char_boundary(start) {
                start
            } else {
                i + rest
                    .char_indices()
                    .nth(9)
                    .map(|(p, _)| p)
                    .unwrap_or(rest.len())
            };
            return match raw[start..].find('>') {
                None => -1,
                Some(p) => {
                    let gt = start + p;
                    self.b.handle_decl(&raw[i + 2..gt]);
                    (gt + 1) as isize
                }
            };
        }
        if rest.starts_with("<![") {
            return match raw[i + 3..].find('>') {
                None => -1,
                Some(p) => {
                    let j = i + 3 + p;
                    if raw.as_bytes()[j - 1] == b']' {
                        self.b.unknown_decl(&raw[i + 3..j - 1]);
                    } else {
                        self.b.handle_comment(&raw[i + 2..j]);
                    }
                    (j + 1) as isize
                }
            };
        }
        self.parse_bogus_comment(raw, i)
    }

    fn parse_comment(&mut self, raw: &str, i: usize) -> isize {
        let from = i + 4;
        let (start, endpos) = match COMMENTCLOSE.find(&raw[from..]) {
            Some(m) => (from + m.start(), from + m.end()),
            None => match COMMENTABRUPTCLOSE.find(&raw[from..]) {
                Some(m) => (from + m.start(), from + m.end()),
                None => return -1,
            },
        };
        self.b.handle_comment(&raw[from..start]);
        endpos as isize
    }

    fn parse_bogus_comment(&mut self, raw: &str, i: usize) -> isize {
        match raw[i + 2..].find('>') {
            None => -1,
            Some(p) => {
                let pos = i + 2 + p;
                self.b.handle_comment(&raw[i + 2..pos]);
                (pos + 1) as isize
            }
        }
    }

    fn parse_pi(&mut self, raw: &str, i: usize) -> isize {
        match raw[i + 2..].find('>') {
            None => -1,
            Some(p) => {
                let j = i + 2 + p;
                self.b.handle_pi(&raw[i + 2..j]);
                (j + 1) as isize
            }
        }
    }

    /// check_for_whole_start_tag / the locatetagend part of parse_endtag: the end, or -1.
    fn locate_tag_end(raw: &str, from: usize) -> isize {
        let end = match LOCATETAGEND.find(&raw[from..]) {
            Ok(Some(m)) => from + m.end(),
            _ => from + 1,
        };
        if raw.as_bytes()[end - 1] != b'>' {
            -1
        } else {
            end as isize
        }
    }

    fn parse_starttag(&mut self, raw: &str, i: usize) -> isize {
        let endpos = Self::locate_tag_end(raw, i + 1);
        if endpos < 0 {
            return endpos;
        }
        let endpos = endpos as usize;
        let (name, mut k) = tagfind(raw, i + 1);
        let tag = pystr::lower(&name);
        let mut attrs: Vec<(String, Option<String>)> = Vec::new();
        while k < endpos {
            let from = prev_char_start(raw, k);
            let caps = match ATTRFIND.captures(&raw[from..]) {
                Ok(Some(c)) => c,
                _ => break,
            };
            let attrname = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let rest = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let mut value: Option<String> = None;
            if !rest.is_empty() {
                let mut v = caps.get(3).map(|m| m.as_str()).unwrap_or("").to_string();
                let quoted = (v.starts_with('\'') && v.ends_with('\''))
                    || (v.starts_with('"') && v.ends_with('"'));
                if quoted && !v.is_empty() {
                    v = if v.len() >= 2 {
                        v[1..v.len() - 1].to_string()
                    } else {
                        String::new()
                    };
                }
                if !v.is_empty() {
                    v = unescape(&v);
                }
                value = Some(v);
            }
            attrs.push((pystr::lower(attrname), value));
            k = from + caps.get(0).map(|m| m.end()).unwrap_or(1);
        }
        let end = pystr::strip(&raw[k.min(endpos)..endpos]);
        if end != ">" && end != "/>" {
            self.b.handle_data(&raw[i..endpos]);
            return endpos as isize;
        }
        if end.ends_with("/>") {
            self.b.handle_startendtag(&tag, attrs);
        } else {
            self.b.handle_starttag(&tag, attrs, true);
            if CDATA_CONTENT_ELEMENTS.contains(&tag.as_str()) || tag == "plaintext" {
                self.set_cdata_mode(&tag, false);
            } else if RCDATA_CONTENT_ELEMENTS.contains(&tag.as_str()) {
                self.set_cdata_mode(&tag, true);
            }
        }
        endpos as isize
    }

    fn parse_endtag(&mut self, raw: &str, i: usize) -> isize {
        if !raw[i + 2..].contains('>') {
            return -1;
        }
        if !is_ascii_letter_at(raw, i + 2) {
            if raw.as_bytes().get(i + 2) == Some(&b'>') {
                return (i + 3) as isize;
            }
            return self.parse_bogus_comment(raw, i);
        }
        let j = Self::locate_tag_end(raw, i + 2);
        if j < 0 {
            return -1;
        }
        let (name, _) = tagfind(raw, i + 2);
        let tag = pystr::lower(&name);
        self.b.handle_endtag(&tag, true);
        self.clear_cdata_mode();
        j
    }
}

/// `BeautifulSoup(html, "html.parser")`.
pub fn parse(html: &str) -> Soup {
    let mut p = HtmlParser {
        b: Builder::new(),
        rawdata: String::new(),
        cdata_elem: None,
        escapable: true,
    };
    // HTMLParser.feed (one call with the whole markup) then close().
    if !html.is_empty() {
        p.rawdata = html.to_string();
        p.goahead(false);
    }
    p.goahead(true);
    // BeautifulSoup._feed: flush the text, close every open tag.
    p.b.end_data(None);
    while p.b.tag_stack.len() > 1 {
        p.b.pop_tag();
    }
    Soup { nodes: p.b.nodes }
}
