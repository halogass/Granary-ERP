//! Port of the parsing half of `email/_header_value_parser.py` (Python 3.12): the token tree, its `str()`
//! and `value`, and the get_*/parse_* functions the header registry uses. Folding is not ported (the ERP
//! never re-serialises parsed headers).
//!
//! Python quirks are kept: errors other than HeaderParseError (IndexError on an empty remainder,
//! AttributeError on a terminal indexed like a list, the defect object `get_domain_literal` appends into the
//! tree, UnboundLocalError in `_get_ptext_to_endchars`) propagate as [`PyErr`] and make the header
//! unreadable, exactly where Python raises.

use once_cell::sync::Lazy;
use regex::Regex;

use super::codecs::{self, DecodeErr, Errors};
use super::pystr::{
    decode_ascii_escaped, encode_ascii_escaped, has_surrogates, is_digit_str, lower, py_int,
    split_ws, strip,
};
use granary_compat::text::pystr as ps;

/// A Python exception that is not HeaderParseError.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyErr {
    pub kind: &'static str,
    pub msg: String,
}

impl PyErr {
    pub fn new(kind: &'static str, msg: impl Into<String>) -> PyErr {
        PyErr {
            kind,
            msg: msg.into(),
        }
    }
    fn index() -> PyErr {
        PyErr::new("IndexError", "string index out of range")
    }
    fn attr(obj: &str, name: &str) -> PyErr {
        PyErr::new(
            "AttributeError",
            format!("'{obj}' object has no attribute '{name}'"),
        )
    }
}

impl std::fmt::Display for PyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for PyErr {}

/// Parse failure: HeaderParseError (caught by the parser's own fallbacks) or any other exception.
#[derive(Clone, Debug)]
pub enum PErr {
    Hpe(String),
    Invalid(String), // _InvalidEwError (a HeaderParseError subclass)
    Py(PyErr),
}

impl From<PyErr> for PErr {
    fn from(e: PyErr) -> PErr {
        PErr::Py(e)
    }
}

type R<T> = Result<T, PErr>;

fn hpe<T>(msg: impl Into<String>) -> R<T> {
    Err(PErr::Hpe(msg.into()))
}

// ─── Character sets ──────────────────────────────────────────────────────────

const SPECIALS: &str = "()<>@,:;.\\\"[]";

fn wsp(c: char) -> bool {
    c == ' ' || c == '\t'
}
fn cfws_leader(c: char) -> bool {
    wsp(c) || c == '('
}
fn specials(c: char) -> bool {
    SPECIALS.contains(c)
}
fn atom_ends(c: char) -> bool {
    specials(c) || wsp(c)
}
fn dot_atom_ends(c: char) -> bool {
    atom_ends(c) && c != '.'
}
fn phrase_ends(c: char) -> bool {
    specials(c) && !matches!(c, '.' | '"' | '(')
}
fn tspecials(c: char) -> bool {
    (specials(c) || matches!(c, '/' | '?' | '=')) && c != '.'
}
fn token_ends(c: char) -> bool {
    tspecials(c) || wsp(c)
}
fn aspecials(c: char) -> bool {
    tspecials(c) || matches!(c, '*' | '\'' | '%')
}
fn attribute_ends(c: char) -> bool {
    aspecials(c) || wsp(c)
}
fn extended_attribute_ends(c: char) -> bool {
    attribute_ends(c) && c != '%'
}

fn first(v: &str) -> Option<char> {
    v.chars().next()
}
/// `value[0]` where Python does not guard it.
fn ix0(v: &str) -> Result<char, PyErr> {
    v.chars().next().ok_or_else(PyErr::index)
}
/// `value[1:]`.
fn rest1(v: &str) -> String {
    let mut it = v.chars();
    it.next();
    it.as_str().to_string()
}
fn starts(v: &str, f: fn(char) -> bool) -> bool {
    first(v).map(f).unwrap_or(false)
}

/// `quote_string(value)`.
pub fn quote_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `_wsp_splitter(value, 1)` joined back: (text before the first run of SP/TAB, the rest from that run).
fn wsp_split(v: &str) -> (String, String) {
    match v.find(wsp) {
        Some(i) => (v[..i].to_string(), v[i..].to_string()),
        None => (v.to_string(), String::new()),
    }
}

// ─── Tokens ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Defect {
    /// errors.InvalidHeaderDefect (the only kind code tests for).
    Invalid,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermKind {
    /// WhiteSpaceTerminal: value ' '.
    Ws,
    /// ValueTerminal: value is the text.
    Val,
    /// EWWhiteSpaceTerminal: value '' and str ''.
    EwWs,
}

#[derive(Clone, Debug)]
pub struct Term {
    pub s: String,
    pub ttype: &'static str,
    pub kind: TermKind,
    pub defects: Vec<Defect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cls {
    TokenList,
    Unstructured,
    Phrase,
    CFWSList,
    Atom,
    Token,
    EncodedWord,
    QuotedString,
    BareQuotedString,
    Comment,
    AddressList,
    Address,
    MailboxList,
    GroupList,
    Group,
    NameAddr,
    AngleAddr,
    ObsRoute,
    Mailbox,
    InvalidMailbox,
    Domain,
    DotAtom,
    DotAtomText,
    NoFoldLiteral,
    AddrSpec,
    ObsLocalPart,
    DisplayName,
    LocalPart,
    DomainLiteral,
    MimeVersion,
    Parameter,
    InvalidParameter,
    Attribute,
    Section,
    Value,
    MimeParameters,
    ContentType,
    ContentDisposition,
    ContentTransferEncoding,
    MsgId,
    MessageId,
    InvalidMessageId,
}

impl Cls {
    fn default_type(self) -> &'static str {
        use Cls::*;
        match self {
            TokenList => "",
            Unstructured => "unstructured",
            Phrase => "phrase",
            CFWSList => "cfws",
            Atom => "atom",
            Token => "token",
            EncodedWord => "encoded-word",
            QuotedString => "quoted-string",
            BareQuotedString => "bare-quoted-string",
            Comment => "comment",
            AddressList => "address-list",
            Address => "address",
            MailboxList => "mailbox-list",
            GroupList => "group-list",
            Group => "group",
            NameAddr => "name-addr",
            AngleAddr => "angle-addr",
            ObsRoute => "obs-route",
            Mailbox => "mailbox",
            InvalidMailbox => "invalid-mailbox",
            Domain => "domain",
            DotAtom => "dot-atom",
            DotAtomText => "dot-atom-text",
            NoFoldLiteral => "no-fold-literal",
            AddrSpec => "addr-spec",
            ObsLocalPart => "obs-local-part",
            DisplayName => "display-name",
            LocalPart => "local-part",
            DomainLiteral => "domain-literal",
            MimeVersion => "mime-version",
            Parameter => "parameter",
            InvalidParameter => "invalid-parameter",
            Attribute => "attribute",
            Section => "section",
            Value => "value",
            MimeParameters => "mime-parameters",
            ContentType => "content-type",
            ContentDisposition => "content-disposition",
            ContentTransferEncoding => "content-transfer-encoding",
            MsgId => "msg-id",
            MessageId => "message-id",
            InvalidMessageId => "invalid-message-id",
        }
    }

    fn py_name(self) -> &'static str {
        use Cls::*;
        match self {
            TokenList => "TokenList",
            Unstructured => "UnstructuredTokenList",
            Phrase => "Phrase",
            CFWSList => "CFWSList",
            Atom => "Atom",
            Token => "Token",
            EncodedWord => "EncodedWord",
            QuotedString => "QuotedString",
            BareQuotedString => "BareQuotedString",
            Comment => "Comment",
            AddressList => "AddressList",
            Address => "Address",
            MailboxList => "MailboxList",
            GroupList => "GroupList",
            Group => "Group",
            NameAddr => "NameAddr",
            AngleAddr => "AngleAddr",
            ObsRoute => "ObsRoute",
            Mailbox => "Mailbox",
            InvalidMailbox => "InvalidMailbox",
            Domain => "Domain",
            DotAtom => "DotAtom",
            DotAtomText => "DotAtomText",
            NoFoldLiteral => "NoFoldLiteral",
            AddrSpec => "AddrSpec",
            ObsLocalPart => "ObsLocalPart",
            DisplayName => "DisplayName",
            LocalPart => "LocalPart",
            DomainLiteral => "DomainLiteral",
            MimeVersion => "MIMEVersion",
            Parameter => "Parameter",
            InvalidParameter => "InvalidParameter",
            Attribute => "Attribute",
            Section => "Section",
            Value => "Value",
            MimeParameters => "MimeParameters",
            ContentType => "ContentType",
            ContentDisposition => "ContentDisposition",
            ContentTransferEncoding => "ContentTransferEncoding",
            MsgId => "MsgID",
            MessageId => "MessageID",
            InvalidMessageId => "InvalidMessageID",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TList {
    pub cls: Cls,
    pub ttype: &'static str,
    pub items: Vec<Tok>,
    pub defects: Vec<Defect>,
    // Parameter attributes
    pub sectioned: bool,
    pub extended: bool,
    pub charset: String,
    // Section.number
    pub number: Option<i128>,
}

#[derive(Clone, Debug)]
pub enum Tok {
    T(Term),
    L(TList),
    /// An `errors.InvalidHeaderDefect` instance appended into the tree by `_check_for_early_dl_end`.
    DefectObj(String),
}

impl TList {
    pub fn new(cls: Cls) -> TList {
        TList {
            cls,
            ttype: cls.default_type(),
            items: Vec::new(),
            defects: Vec::new(),
            sectioned: false,
            extended: false,
            charset: "us-ascii".into(),
            number: None,
        }
    }
    fn with(cls: Cls, items: Vec<Tok>) -> TList {
        let mut t = TList::new(cls);
        t.items = items;
        t
    }
    fn push(&mut self, t: impl Into<Tok>) {
        self.items.push(t.into());
    }
    fn prepend(&mut self, t: impl Into<Tok>) {
        self.items.insert(0, t.into());
    }
}

impl From<Term> for Tok {
    fn from(t: Term) -> Tok {
        Tok::T(t)
    }
}
impl From<TList> for Tok {
    fn from(t: TList) -> Tok {
        Tok::L(t)
    }
}

fn vt(s: impl Into<String>, ttype: &'static str) -> Term {
    Term {
        s: s.into(),
        ttype,
        kind: TermKind::Val,
        defects: Vec::new(),
    }
}
fn wt(s: impl Into<String>, ttype: &'static str) -> Term {
    Term {
        s: s.into(),
        ttype,
        kind: TermKind::Ws,
        defects: Vec::new(),
    }
}
fn dot() -> Term {
    vt(".", "dot")
}
fn list_separator() -> Term {
    vt(",", "list-separator")
}

/// `token[:0] = [leader]` on a token that may be a terminal (TypeError in Python).
fn prepend_into(tok: &mut Tok, leader: Tok) -> Result<(), PyErr> {
    match tok {
        Tok::L(l) => {
            l.items.insert(0, leader);
            Ok(())
        }
        Tok::T(_) => Err(PyErr::new(
            "TypeError",
            "'ValueTerminal' object does not support item assignment",
        )),
        Tok::DefectObj(_) => Err(PyErr::new(
            "TypeError",
            "'InvalidHeaderDefect' object does not support item assignment",
        )),
    }
}

impl Tok {
    pub fn ttype(&self) -> Result<&'static str, PyErr> {
        match self {
            Tok::T(t) => Ok(t.ttype),
            Tok::L(l) => Ok(l.ttype),
            Tok::DefectObj(_) => Err(PyErr::attr("InvalidHeaderDefect", "token_type")),
        }
    }

    /// `str(token)`.
    pub fn to_str(&self) -> Result<String, PyErr> {
        match self {
            Tok::T(t) => Ok(if t.kind == TermKind::EwWs {
                String::new()
            } else {
                t.s.clone()
            }),
            Tok::L(l) => l.to_str(),
            Tok::DefectObj(m) => Ok(m.clone()),
        }
    }

    /// `token.value`.
    pub fn value(&self) -> Result<String, PyErr> {
        match self {
            Tok::T(t) => Ok(match t.kind {
                TermKind::Ws => " ".into(),
                TermKind::Val => t.s.clone(),
                TermKind::EwWs => String::new(),
            }),
            Tok::L(l) => l.value(),
            Tok::DefectObj(_) => Err(PyErr::attr("InvalidHeaderDefect", "value")),
        }
    }

    /// `token.all_defects`.
    pub fn all_defects(&self) -> Result<Vec<Defect>, PyErr> {
        match self {
            Tok::T(t) => Ok(t.defects.clone()),
            Tok::L(l) => l.all_defects(),
            Tok::DefectObj(_) => Err(PyErr::attr("InvalidHeaderDefect", "all_defects")),
        }
    }

    /// `token[i]` on something that is expected to be a TokenList; a terminal yields a plain str (whose
    /// `.token_type` then raises AttributeError) - modelled as [`Index::Str`].
    fn index(&self, i: isize) -> Result<Index<'_>, PyErr> {
        match self {
            Tok::L(l) => {
                let n = l.items.len() as isize;
                let j = if i < 0 { n + i } else { i };
                if j < 0 || j >= n {
                    return Err(PyErr::new("IndexError", "list index out of range"));
                }
                Ok(Index::Tok(&l.items[j as usize]))
            }
            Tok::T(t) => {
                let n = t.s.chars().count() as isize;
                let j = if i < 0 { n + i } else { i };
                if j < 0 || j >= n {
                    return Err(PyErr::index());
                }
                Ok(Index::Str)
            }
            Tok::DefectObj(_) => Err(PyErr::new(
                "TypeError",
                "'InvalidHeaderDefect' object is not subscriptable",
            )),
        }
    }

    fn as_list(&self) -> Option<&TList> {
        match self {
            Tok::L(l) => Some(l),
            _ => None,
        }
    }
}

enum Index<'a> {
    Tok(&'a Tok),
    Str,
}

impl Index<'_> {
    fn ttype(&self) -> Result<&'static str, PyErr> {
        match self {
            Index::Tok(t) => t.ttype(),
            Index::Str => Err(PyErr::attr("str", "token_type")),
        }
    }
}

impl TList {
    fn class_err(&self, name: &str) -> PyErr {
        PyErr::attr(self.cls.py_name(), name)
    }

    fn join_str(&self) -> Result<String, PyErr> {
        let mut s = String::new();
        for x in &self.items {
            s.push_str(&x.to_str()?);
        }
        Ok(s)
    }

    fn join_value(&self) -> Result<String, PyErr> {
        let mut s = String::new();
        for x in &self.items {
            s.push_str(&x.value()?);
        }
        Ok(s)
    }

    /// `str(token)`.
    pub fn to_str(&self) -> Result<String, PyErr> {
        match self.cls {
            Cls::BareQuotedString => Ok(quote_string(&self.join_str()?)),
            Cls::Comment => {
                let mut s = String::from("(");
                for x in &self.items {
                    if x.ttype()? == "comment" {
                        s.push_str(&x.to_str()?);
                    } else {
                        s.push_str(
                            &x.to_str()?
                                .replace('\\', "\\\\")
                                .replace('(', "\\(")
                                .replace(')', "\\)"),
                        );
                    }
                }
                s.push(')');
                Ok(s)
            }
            Cls::MimeParameters => {
                let mut parts: Vec<String> = Vec::new();
                for (name, value) in self.mime_params()? {
                    if !value.is_empty() {
                        parts.push(format!("{}={}", name, quote_string(&value)));
                    } else {
                        parts.push(name);
                    }
                }
                let joined = parts.join("; ");
                Ok(if joined.is_empty() {
                    String::new()
                } else {
                    format!(" {joined}")
                })
            }
            _ => self.join_str(),
        }
    }

    /// `token.value`.
    pub fn value(&self) -> Result<String, PyErr> {
        match self.cls {
            Cls::CFWSList | Cls::Comment => {
                // WhiteSpaceTokenList.value is ' ' (the children are never looked at).
                Ok(" ".into())
            }
            Cls::BareQuotedString => self.join_str(),
            Cls::DisplayName => self.display_name_value(),
            Cls::AddrSpec => {
                if self.items.len() < 3 {
                    return self.items[0].value();
                }
                Ok(format!(
                    "{}{}{}",
                    ps::rstrip(&self.items[0].value()?),
                    self.items[1].value()?,
                    ps::lstrip(&self.items[2].value()?)
                ))
            }
            Cls::LocalPart => {
                let t0 = &self.items[0];
                if t0.ttype()? == "quoted-string" {
                    t0.as_list().expect("quoted-string").quoted_value()
                } else {
                    t0.value()
                }
            }
            _ => self.join_value(),
        }
    }

    pub fn all_defects(&self) -> Result<Vec<Defect>, PyErr> {
        let mut out = self.defects.clone();
        for x in &self.items {
            out.extend(x.all_defects()?);
        }
        Ok(out)
    }

    // QuotedString
    fn quoted_value(&self) -> Result<String, PyErr> {
        let mut s = String::new();
        for x in &self.items {
            if x.ttype()? == "bare-quoted-string" {
                s.push_str(&x.to_str()?);
            } else {
                s.push_str(&x.value()?);
            }
        }
        Ok(s)
    }

    fn quoted_stripped_value(&self) -> Result<Option<String>, PyErr> {
        for x in &self.items {
            if x.ttype()? == "bare-quoted-string" {
                return x.value().map(Some);
            }
        }
        Ok(None)
    }

    // DisplayName ---------------------------------------------------------

    fn display_name_value(&self) -> Result<String, PyErr> {
        let mut quote = !self.defects.is_empty();
        if !quote {
            for x in &self.items {
                if x.ttype()? == "quoted-string" {
                    quote = true;
                }
            }
        }
        if !self.items.is_empty() && quote {
            let first_tok = &self.items[0];
            let last_tok = &self.items[self.items.len() - 1];
            let pre = if first_tok.ttype()? == "cfws" || first_tok.index(0)?.ttype()? == "cfws" {
                " "
            } else {
                ""
            };
            let post = if last_tok.ttype()? == "cfws" || last_tok.index(-1)?.ttype()? == "cfws" {
                " "
            } else {
                ""
            };
            Ok(format!(
                "{}{}{}",
                pre,
                quote_string(&self.dn_display_name()?),
                post
            ))
        } else {
            self.join_value()
        }
    }

    /// DisplayName.display_name.
    fn dn_display_name(&self) -> Result<String, PyErr> {
        let mut res: Vec<Tok> = self.items.clone();
        if res.is_empty() {
            return Ok(String::new());
        }
        if res[0].ttype()? == "cfws" {
            res.remove(0);
        } else if res[0].index(0)?.ttype()? == "cfws" {
            let inner = res[0].as_list().expect("indexed").items[1..].to_vec();
            res[0] = Tok::L(TList::with(Cls::TokenList, inner));
        }
        let last = res
            .last()
            .ok_or_else(|| PyErr::new("IndexError", "list index out of range"))?;
        if last.ttype()? == "cfws" {
            res.pop();
        } else if last.index(-1)?.ttype()? == "cfws" {
            let items = &last.as_list().expect("indexed").items;
            let inner = items[..items.len() - 1].to_vec();
            let n = res.len();
            res[n - 1] = Tok::L(TList::with(Cls::TokenList, inner));
        }
        TList::with(Cls::TokenList, res).join_value()
    }

    // Address-level properties ---------------------------------------------

    /// `.display_name` (None when Python returns None; AttributeError for classes without it).
    pub fn display_name(&self) -> Result<Option<String>, PyErr> {
        match self.cls {
            Cls::DisplayName => self.dn_display_name().map(Some),
            Cls::Address => {
                if self.items[0].ttype()? == "group" {
                    self.items[0].as_list().expect("group").display_name()
                } else {
                    Ok(None)
                }
            }
            Cls::Group => self.items[0]
                .as_list()
                .ok_or_else(|| self.class_err("display_name"))?
                .display_name(),
            Cls::NameAddr => {
                if self.items.len() == 1 {
                    return Ok(None);
                }
                self.items[0]
                    .as_list()
                    .ok_or_else(|| PyErr::attr("ValueTerminal", "display_name"))?
                    .display_name()
            }
            Cls::Mailbox => {
                if self.items[0].ttype()? == "name-addr" {
                    self.items[0].as_list().expect("name-addr").display_name()
                } else {
                    Ok(None)
                }
            }
            Cls::InvalidMailbox => Ok(None),
            _ => Err(self.class_err("display_name")),
        }
    }

    pub fn local_part(&self) -> Result<Option<String>, PyErr> {
        match self.cls {
            Cls::Mailbox => sub(&self.items[0], |l| l.local_part(), "local_part"),
            Cls::InvalidMailbox => Ok(None),
            Cls::NameAddr => sub(
                &self.items[self.items.len() - 1],
                |l| l.local_part(),
                "local_part",
            ),
            Cls::AngleAddr => {
                for x in &self.items {
                    if x.ttype()? == "addr-spec" {
                        return x.as_list().expect("addr-spec").local_part();
                    }
                }
                Ok(None)
            }
            Cls::AddrSpec => sub(&self.items[0], |l| l.local_part(), "local_part"),
            Cls::LocalPart => self.lp_local_part().map(Some),
            _ => Err(self.class_err("local_part")),
        }
    }

    pub fn domain(&self) -> Result<Option<String>, PyErr> {
        match self.cls {
            Cls::Mailbox => sub(&self.items[0], |l| l.domain(), "domain"),
            Cls::InvalidMailbox => Ok(None),
            Cls::NameAddr => sub(&self.items[self.items.len() - 1], |l| l.domain(), "domain"),
            Cls::AngleAddr => {
                for x in &self.items {
                    if x.ttype()? == "addr-spec" {
                        return x.as_list().expect("addr-spec").domain();
                    }
                }
                Ok(None)
            }
            Cls::AddrSpec => {
                if self.items.len() < 3 {
                    return Ok(None);
                }
                sub(&self.items[self.items.len() - 1], |l| l.domain(), "domain")
            }
            Cls::Domain | Cls::DomainLiteral => Ok(Some(split_ws(&self.join_value()?).concat())),
            _ => Err(self.class_err("domain")),
        }
    }

    /// LocalPart.local_part: whitespace stripped from the ends and around dots.
    fn lp_local_part(&self) -> Result<String, PyErr> {
        let mut seq: Vec<Tok> = match &self.items[0] {
            Tok::L(l) => l.items.clone(),
            Tok::T(_) => {
                return Err(PyErr::new(
                    "TypeError",
                    "can only concatenate str (not \"list\") to str",
                ))
            }
            Tok::DefectObj(_) => return Err(PyErr::new("TypeError", "unsupported operand")),
        };
        seq.push(Tok::T(dot()));
        let mut res: Vec<Tok> = vec![Tok::T(dot())];
        let mut last_is_tl = false;
        for tok in seq {
            if tok.ttype()? == "cfws" {
                continue;
            }
            let last = res.last().unwrap().clone();
            if last_is_tl && tok.ttype()? == "dot" && last.index(-1)?.ttype()? == "cfws" {
                let items = &last.as_list().expect("tl").items;
                let n = res.len();
                res[n - 1] = Tok::L(TList::with(
                    Cls::TokenList,
                    items[..items.len() - 1].to_vec(),
                ));
            }
            let is_tl = matches!(tok, Tok::L(_));
            let last = res.last().unwrap();
            if is_tl && last.ttype()? == "dot" && tok.index(0)?.ttype()? == "cfws" {
                let items = &tok.as_list().expect("tl").items;
                res.push(Tok::L(TList::with(Cls::TokenList, items[1..].to_vec())));
            } else {
                res.push(tok);
            }
            last_is_tl = is_tl;
        }
        let n = res.len();
        TList::with(Cls::TokenList, res[1..n - 1].to_vec()).join_value()
    }

    /// AddressList.addresses
    pub fn addresses(&self) -> Result<Vec<&TList>, PyErr> {
        let mut out = Vec::new();
        for x in &self.items {
            if x.ttype()? == "address" {
                out.push(x.as_list().expect("address"));
            }
        }
        Ok(out)
    }

    /// `.all_mailboxes` for Address / Group / GroupList / MailboxList.
    pub fn all_mailboxes(&self) -> Result<Vec<&TList>, PyErr> {
        match self.cls {
            Cls::Address => {
                let t0 = &self.items[0];
                let tt = t0.ttype()?;
                if tt == "mailbox" || tt == "invalid-mailbox" {
                    return Ok(vec![t0
                        .as_list()
                        .ok_or_else(|| PyErr::attr("ValueTerminal", "all_mailboxes"))?]);
                }
                t0.as_list()
                    .ok_or_else(|| PyErr::attr("ValueTerminal", "all_mailboxes"))?
                    .all_mailboxes()
            }
            Cls::Group => {
                let t2 = self
                    .items
                    .get(2)
                    .ok_or_else(|| PyErr::new("IndexError", "list index out of range"))?;
                if t2.ttype()? != "group-list" {
                    return Ok(Vec::new());
                }
                t2.as_list().expect("group-list").all_mailboxes()
            }
            Cls::GroupList => {
                if self.items.is_empty() || self.items[0].ttype()? != "mailbox-list" {
                    return Ok(Vec::new());
                }
                self.items[0]
                    .as_list()
                    .expect("mailbox-list")
                    .all_mailboxes()
            }
            Cls::MailboxList => {
                let mut out = Vec::new();
                for x in &self.items {
                    let tt = x.ttype()?;
                    if tt == "mailbox" || tt == "invalid-mailbox" {
                        out.push(
                            x.as_list()
                                .ok_or_else(|| PyErr::attr("ValueTerminal", "x"))?,
                        );
                    }
                }
                Ok(out)
            }
            _ => Err(self.class_err("all_mailboxes")),
        }
    }

    // Parameters -----------------------------------------------------------

    fn section_number(&self) -> Result<i128, PyErr> {
        if self.sectioned {
            match &self.items[1] {
                Tok::L(l) => Ok(l.number.unwrap_or(0)),
                _ => Err(PyErr::attr("ValueTerminal", "number")),
            }
        } else {
            Ok(0)
        }
    }

    fn stripped_value(&self) -> Result<Option<String>, PyErr> {
        match self.cls {
            Cls::Value => {
                let mut token = &self.items[0];
                if token.ttype()? == "cfws" {
                    token = &self.items[1];
                }
                let tt = token.ttype()?;
                if tt.ends_with("quoted-string")
                    || tt.ends_with("attribute")
                    || tt.ends_with("extended-attribute")
                {
                    return token
                        .as_list()
                        .ok_or_else(|| PyErr::attr("ValueTerminal", "stripped_value"))?
                        .stripped_value();
                }
                self.value().map(Some)
            }
            Cls::QuotedString | Cls::BareQuotedString => self.quoted_stripped_value(),
            Cls::Attribute => {
                for t in &self.items {
                    if t.ttype()?.ends_with("attrtext") {
                        return t.value().map(Some);
                    }
                }
                Ok(None)
            }
            _ => Err(self.class_err("stripped_value")),
        }
    }

    fn param_value(&self) -> Result<Option<String>, PyErr> {
        for token in &self.items {
            let tt = token.ttype()?;
            if tt == "value" {
                return token.as_list().expect("value").stripped_value();
            }
            if tt == "quoted-string" {
                for t2 in &token.as_list().expect("qs").items {
                    if t2.ttype()? == "bare-quoted-string" {
                        for t3 in &t2.as_list().expect("bqs").items {
                            if t3.ttype()? == "value" {
                                return t3.as_list().expect("value").stripped_value();
                            }
                        }
                    }
                }
            }
        }
        Ok(Some(String::new()))
    }

    /// MimeParameters.params: (name, value) in first-seen order.
    pub fn mime_params(&self) -> Result<Vec<(String, String)>, PyErr> {
        let mut params: Vec<(String, Vec<(i128, &TList)>)> = Vec::new();
        for token in &self.items {
            if !token.ttype()?.ends_with("parameter") {
                continue;
            }
            let l = token
                .as_list()
                .ok_or_else(|| PyErr::attr("ValueTerminal", "x"))?;
            if l.items.is_empty() {
                return Err(PyErr::new("IndexError", "list index out of range"));
            }
            if l.items[0].ttype()? != "attribute" {
                continue;
            }
            let name = strip(&l.items[0].value()?).to_string();
            let sn = l.section_number()?;
            match params.iter_mut().find(|(n, _)| *n == name) {
                Some((_, v)) => v.push((sn, l)),
                None => params.push((name, vec![(sn, l)])),
            }
        }
        let mut out = Vec::new();
        for (name, mut parts) in params {
            parts.sort_by_key(|p| p.0); // stable, like sorted(key=itemgetter(0))
            let first_param = parts[0].1;
            let charset = first_param.charset.clone();
            if !first_param.extended && parts.len() > 1 && parts[1].0 == 0 {
                parts.truncate(1);
            }
            let mut value_parts: Vec<String> = Vec::new();
            let mut i: i128 = 0;
            for (section_number, param) in parts {
                if section_number != i && !param.extended {
                    continue;
                }
                i += 1;
                let mut value = param.param_value()?.ok_or_else(|| {
                    PyErr::new(
                        "TypeError",
                        "sequence item 0: expected str instance, NoneType found",
                    )
                })?;
                if param.extended {
                    match codecs::unquote_to_bytes(&value) {
                        Err(()) => value = codecs::unquote_latin1(&value),
                        Ok(bytes) => {
                            value = match codecs::decode(&bytes, &charset, Errors::SurrogateEscape)
                            {
                                Ok(v) => v,
                                Err(DecodeErr::Lookup(_)) | Err(DecodeErr::Encode(_)) => {
                                    codecs::decode(&bytes, "us-ascii", Errors::SurrogateEscape)
                                        .map_err(|e| {
                                            PyErr::new("UnicodeDecodeError", e.to_string())
                                        })?
                                }
                                Err(DecodeErr::Decode(m)) => {
                                    return Err(PyErr::new("UnicodeDecodeError", m))
                                }
                                Err(DecodeErr::Value(m)) => {
                                    return Err(PyErr::new("ValueError", m))
                                }
                            };
                        }
                    }
                }
                value_parts.push(value);
            }
            out.push((name, value_parts.concat()));
        }
        Ok(out)
    }
}

fn sub(
    t: &Tok,
    f: impl Fn(&TList) -> Result<Option<String>, PyErr>,
    name: &str,
) -> Result<Option<String>, PyErr> {
    match t {
        Tok::L(l) => f(l),
        Tok::T(_) => Err(PyErr::attr("ValueTerminal", name)),
        Tok::DefectObj(_) => Err(PyErr::attr("InvalidHeaderDefect", name)),
    }
}

// ─── Parser ──────────────────────────────────────────────────────────────────

/// `_get_ptext_to_endchars(value, endchars)`: (unquoted text, rest, had quoted pairs).
fn get_ptext_to_endchars(value: &str, endchars: &str) -> Result<(String, String, bool), PyErr> {
    let (fragment, remainder) = wsp_split(value);
    let chars: Vec<char> = fragment.chars().collect();
    if chars.is_empty() {
        // `for pos in range(0)` never binds pos; the for/else then reads it.
        return Err(PyErr::new(
            "UnboundLocalError",
            "cannot access local variable 'pos' where it is not associated with a value",
        ));
    }
    let mut vchars = String::new();
    let mut escape = false;
    let mut had_qp = false;
    let mut pos = 0;
    let mut broke = false;
    while pos < chars.len() {
        let c = chars[pos];
        if c == '\\' {
            if escape {
                escape = false;
                had_qp = true;
            } else {
                escape = true;
                pos += 1;
                continue;
            }
        }
        if escape {
            escape = false;
        } else if endchars.contains(c) {
            broke = true;
            break;
        }
        vchars.push(c);
        pos += 1;
    }
    let pos = if broke { pos } else { chars.len() };
    let rest: String = chars[pos..].iter().collect::<String>() + &remainder;
    Ok((vchars, rest, had_qp))
}

/// `get_fws(value)`.
fn get_fws(value: &str) -> (Term, String) {
    let newvalue = ps::lstrip(value);
    let ws = &value[..value.len() - newvalue.len()];
    (wt(ws, "fws"), newvalue.to_string())
}

static Q_BYTE: Lazy<regex::bytes::Regex> =
    Lazy::new(|| regex::bytes::Regex::new(r"=([a-fA-F0-9]{2})").unwrap());

/// `_encoded_words.decode(ew)`: (text, charset, lang); Err is ValueError/KeyError.
fn ew_decode(ew: &str) -> Result<(String, String, String), String> {
    let parts: Vec<&str> = ew.split('?').collect();
    if parts.len() != 5 {
        return Err("ValueError: not enough values to unpack".into());
    }
    let (charset, lang) = match parts[1].split_once('*') {
        Some((c, l)) => (c.to_string(), l.to_string()),
        None => (parts[1].to_string(), String::new()),
    };
    let cte = lower(parts[2]);
    let bstring = encode_ascii_escaped(parts[3]).map_err(|_| "UnicodeEncodeError".to_string())?;
    let bstring = match cte.as_str() {
        "q" => {
            let replaced: Vec<u8> = bstring
                .iter()
                .map(|&b| if b == b'_' { b' ' } else { b })
                .collect();
            Q_BYTE
                .replace_all(&replaced, |c: &regex::bytes::Captures| {
                    let h = std::str::from_utf8(&c[1]).unwrap();
                    vec![u8::from_str_radix(h, 16).unwrap()]
                })
                .into_owned()
        }
        "b" => codecs::decode_b(&bstring),
        _ => return Err(format!("KeyError: '{cte}'")),
    };
    let string = match codecs::decode(&bstring, &charset, Errors::Strict) {
        Ok(s) => s,
        Err(DecodeErr::Decode(_)) => codecs::decode(&bstring, &charset, Errors::SurrogateEscape)
            .map_err(|e| e.to_string())?,
        Err(DecodeErr::Lookup(_)) | Err(DecodeErr::Encode(_)) => {
            codecs::decode(&bstring, "ascii", Errors::SurrogateEscape).map_err(|e| e.to_string())?
        }
        Err(DecodeErr::Value(m)) => return Err(m),
    };
    Ok((string, charset, lang))
}

fn hexdigit(c: char) -> bool {
    c.is_ascii_hexdigit()
}

/// `get_encoded_word(value)`.
fn get_encoded_word(value: &str) -> R<(TList, String)> {
    let mut ew = TList::new(Cls::EncodedWord);
    if !value.starts_with("=?") {
        return hpe("expected encoded word");
    }
    let after = &value[2..];
    let (mut tok, mut remainder): (String, Option<String>) = match after.split_once("?=") {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (after.to_string(), None),
    };
    if remainder.is_none() {
        return hpe("expected encoded word");
    }
    let remstr = remainder.clone().unwrap_or_default();
    let rc: Vec<char> = remstr.chars().take(2).collect();
    if rc.len() > 1 && hexdigit(rc[0]) && hexdigit(rc[1]) && tok.matches('?').count() < 2 {
        let (rest, rem2) = match remstr.split_once("?=") {
            Some((a, b)) => (a.to_string(), Some(b.to_string())),
            None => (remstr.clone(), None),
        };
        tok = format!("{tok}?={rest}");
        remainder = rem2;
    }
    if split_ws(&tok).len() > 1 {
        ew.defects.push(Defect::Invalid);
    }
    let value = remainder.unwrap_or_default();
    let (mut text, _charset, _lang) = ew_decode(&format!("=?{tok}?="))
        .map_err(|e| PErr::Invalid(format!("encoded word format invalid: {e}")))?;
    while !text.is_empty() {
        if starts(&text, wsp) {
            let (token, rest) = get_fws(&text);
            ew.push(token);
            text = rest;
            continue;
        }
        let (chars, rest) = wsp_split(&text);
        ew.push(vt(chars, "vtext"));
        text = rest;
    }
    if !value.is_empty() && !starts(&value, wsp) {
        ew.defects.push(Defect::Invalid);
    }
    Ok((ew, value))
}

static RFC2047: Lazy<Regex> = Lazy::new(|| Regex::new(r"=\?[^?]*\?[qQbB]\?.*?\?=").unwrap());

/// `get_unstructured(value)`.
pub fn get_unstructured(value: &str) -> TList {
    let mut unstructured = TList::new(Cls::Unstructured);
    let mut value = value.to_string();
    while !value.is_empty() {
        if starts(&value, wsp) {
            let (token, rest) = get_fws(&value);
            unstructured.push(token);
            value = rest;
            continue;
        }
        let mut valid_ew = true;
        if value.starts_with("=?") {
            match get_encoded_word(&value) {
                Err(PErr::Invalid(_)) => valid_ew = false,
                Err(_) => {}
                Ok((token, rest)) => {
                    let mut have_ws = true;
                    if !unstructured.items.is_empty()
                        && unstructured.items.last().unwrap().ttype().unwrap_or("") != "fws"
                    {
                        unstructured.defects.push(Defect::Invalid);
                        have_ws = false;
                    }
                    if have_ws && unstructured.items.len() > 1 {
                        let n = unstructured.items.len();
                        if unstructured.items[n - 2].ttype().unwrap_or("") == "encoded-word" {
                            if let Tok::T(t) = &unstructured.items[n - 1] {
                                let mut t = t.clone();
                                t.kind = TermKind::EwWs;
                                t.ttype = "fws";
                                unstructured.items[n - 1] = Tok::T(t);
                            }
                        }
                    }
                    unstructured.push(token);
                    value = rest;
                    continue;
                }
            }
        }
        let (mut tok, mut rest) = wsp_split(&value);
        if valid_ew && RFC2047.is_match(&tok) {
            match value.find("=?") {
                Some(i) => {
                    tok = value[..i].to_string();
                    rest = value[i..].to_string();
                }
                None => {
                    tok = value.clone();
                    rest = String::new();
                }
            }
        }
        unstructured.push(vt(tok, "vtext"));
        value = rest;
    }
    unstructured
}

fn get_qp_ctext(value: &str) -> R<(Term, String)> {
    let (ptext, rest, _) = get_ptext_to_endchars(value, "()")?;
    Ok((wt(ptext, "ptext"), rest))
}

fn get_qcontent(value: &str) -> R<(Term, String)> {
    let (ptext, rest, _) = get_ptext_to_endchars(value, "\"")?;
    Ok((vt(ptext, "ptext"), rest))
}

fn get_atext(value: &str) -> R<(Term, String)> {
    let end = value.find(atom_ends).unwrap_or(value.len());
    if end == 0 {
        return hpe("expected atext");
    }
    Ok((vt(&value[..end], "atext"), value[end..].to_string()))
}

fn get_bare_quoted_string(value: &str) -> R<(TList, String)> {
    if ix0(value)? != '"' {
        return hpe("expected '\"'");
    }
    let mut bqs = TList::new(Cls::BareQuotedString);
    let mut value = rest1(value);
    if first(&value) == Some('"') {
        let (token, rest) = get_qcontent(&value)?;
        bqs.push(token);
        value = rest;
    }
    while !value.is_empty() && first(&value) != Some('"') {
        let token: Tok;
        if starts(&value, wsp) {
            let (t, rest) = get_fws(&value);
            token = t.into();
            value = rest;
        } else if value.starts_with("=?") {
            let mut valid_ew = false;
            match get_encoded_word(&value) {
                Ok((t, rest)) => {
                    bqs.defects.push(Defect::Invalid);
                    valid_ew = true;
                    token = t.into();
                    value = rest;
                }
                Err(PErr::Py(e)) => return Err(PErr::Py(e)),
                Err(_) => {
                    let (t, rest) = get_qcontent(&value)?;
                    token = t.into();
                    value = rest;
                }
            }
            if valid_ew && bqs.items.len() > 1 {
                let n = bqs.items.len();
                if bqs.items[n - 1].ttype()? == "fws" && bqs.items[n - 2].ttype()? == "encoded-word"
                {
                    if let Tok::T(t) = &bqs.items[n - 1] {
                        let mut t = t.clone();
                        t.kind = TermKind::EwWs;
                        bqs.items[n - 1] = Tok::T(t);
                    }
                }
            }
        } else {
            let (t, rest) = get_qcontent(&value)?;
            token = t.into();
            value = rest;
        }
        bqs.items.push(token);
    }
    if value.is_empty() {
        bqs.defects.push(Defect::Invalid);
        return Ok((bqs, value));
    }
    Ok((bqs, rest1(&value)))
}

fn get_comment(value: &str) -> R<(TList, String)> {
    if !value.is_empty() && first(value) != Some('(') {
        return hpe("expected '('");
    }
    let mut comment = TList::new(Cls::Comment);
    let mut value = rest1(value);
    while !value.is_empty() && first(&value) != Some(')') {
        if starts(&value, wsp) {
            let (t, rest) = get_fws(&value);
            comment.push(t);
            value = rest;
        } else if first(&value) == Some('(') {
            let (t, rest) = get_comment(&value)?;
            comment.push(t);
            value = rest;
        } else {
            let (t, rest) = get_qp_ctext(&value)?;
            comment.push(t);
            value = rest;
        }
    }
    if value.is_empty() {
        comment.defects.push(Defect::Invalid);
        return Ok((comment, value));
    }
    Ok((comment, rest1(&value)))
}

fn get_cfws(value: &str) -> R<(TList, String)> {
    let mut cfws = TList::new(Cls::CFWSList);
    let mut value = value.to_string();
    while starts(&value, cfws_leader) {
        if starts(&value, wsp) {
            let (t, rest) = get_fws(&value);
            cfws.push(t);
            value = rest;
        } else {
            let (t, rest) = get_comment(&value)?;
            cfws.push(t);
            value = rest;
        }
    }
    Ok((cfws, value))
}

fn get_quoted_string(value: &str) -> R<(TList, String)> {
    let mut qs = TList::new(Cls::QuotedString);
    let mut value = value.to_string();
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        qs.push(t);
        value = rest;
    }
    let (t, rest) = get_bare_quoted_string(&value)?;
    qs.push(t);
    value = rest;
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        qs.push(t);
        value = rest;
    }
    Ok((qs, value))
}

fn get_atom(value: &str) -> R<(TList, String)> {
    let mut atom = TList::new(Cls::Atom);
    let mut value = value.to_string();
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        atom.push(t);
        value = rest;
    }
    if starts(&value, atom_ends) {
        return hpe("expected atom");
    }
    if value.starts_with("=?") {
        match get_encoded_word(&value) {
            Ok((t, rest)) => {
                atom.push(t);
                value = rest;
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => {
                let (t, rest) = get_atext(&value)?;
                atom.push(t);
                value = rest;
            }
        }
    } else {
        let (t, rest) = get_atext(&value)?;
        atom.push(t);
        value = rest;
    }
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        atom.push(t);
        value = rest;
    }
    Ok((atom, value))
}

fn get_dot_atom_text(value: &str) -> R<(TList, String)> {
    let mut dat = TList::new(Cls::DotAtomText);
    if value.is_empty() || starts(value, atom_ends) {
        return hpe("expected atom at a start of dot-atom-text");
    }
    let mut value = value.to_string();
    while !value.is_empty() && !starts(&value, atom_ends) {
        let (t, rest) = get_atext(&value)?;
        dat.push(t);
        value = rest;
        if first(&value) == Some('.') {
            dat.push(dot());
            value = rest1(&value);
        }
    }
    if dat
        .items
        .last()
        .map(|t| t.ttype().unwrap_or("") == "dot")
        .unwrap_or(false)
    {
        return hpe("expected atom at end of dot-atom-text");
    }
    Ok((dat, value))
}

fn get_dot_atom(value: &str) -> R<(TList, String)> {
    let mut da = TList::new(Cls::DotAtom);
    let mut value = value.to_string();
    if cfws_leader(ix0(&value)?) {
        let (t, rest) = get_cfws(&value)?;
        da.push(t);
        value = rest;
    }
    if value.starts_with("=?") {
        match get_encoded_word(&value) {
            Ok((t, rest)) => {
                da.push(t);
                value = rest;
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => {
                let (t, rest) = get_dot_atom_text(&value)?;
                da.push(t);
                value = rest;
            }
        }
    } else {
        let (t, rest) = get_dot_atom_text(&value)?;
        da.push(t);
        value = rest;
    }
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        da.push(t);
        value = rest;
    }
    Ok((da, value))
}

fn get_word(value: &str) -> R<(TList, String)> {
    let mut value = value.to_string();
    let mut leader = None;
    if cfws_leader(ix0(&value)?) {
        let (l, rest) = get_cfws(&value)?;
        leader = Some(l);
        value = rest;
    }
    if value.is_empty() {
        return hpe("Expected 'atom' or 'quoted-string' but found nothing.");
    }
    let (mut token, rest) = if first(&value) == Some('"') {
        get_quoted_string(&value)?
    } else if starts(&value, specials) {
        return hpe("Expected 'atom' or 'quoted-string'");
    } else {
        get_atom(&value)?
    };
    if let Some(l) = leader {
        token.prepend(l);
    }
    Ok((token, rest))
}

fn get_phrase(value: &str) -> R<(TList, String)> {
    let mut phrase = TList::new(Cls::Phrase);
    let mut value = value.to_string();
    match get_word(&value) {
        Ok((t, rest)) => {
            phrase.push(t);
            value = rest;
        }
        Err(PErr::Py(e)) => return Err(PErr::Py(e)),
        Err(_) => phrase.defects.push(Defect::Invalid),
    }
    while !value.is_empty() && !starts(&value, phrase_ends) {
        if first(&value) == Some('.') {
            phrase.push(dot());
            phrase.defects.push(Defect::Other);
            value = rest1(&value);
        } else {
            match get_word(&value) {
                Ok((t, rest)) => {
                    phrase.push(t);
                    value = rest;
                }
                Err(PErr::Py(e)) => return Err(PErr::Py(e)),
                Err(e) => {
                    if starts(&value, cfws_leader) {
                        let (t, rest) = get_cfws(&value)?;
                        phrase.defects.push(Defect::Other);
                        phrase.push(t);
                        value = rest;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }
    Ok((phrase, value))
}

fn get_local_part(value: &str) -> R<(TList, String)> {
    let mut local_part = TList::new(Cls::LocalPart);
    let mut value = value.to_string();
    let mut leader = None;
    if cfws_leader(ix0(&value)?) {
        let (l, rest) = get_cfws(&value)?;
        leader = Some(l);
        value = rest;
    }
    if value.is_empty() {
        return hpe("expected local-part");
    }
    let mut token: TList = match get_dot_atom(&value) {
        Ok((t, rest)) => {
            value = rest;
            t
        }
        Err(PErr::Py(e)) => return Err(PErr::Py(e)),
        Err(_) => match get_word(&value) {
            Ok((t, rest)) => {
                value = rest;
                t
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(e) => {
                let c = ix0(&value)?;
                if c != '\\' && phrase_ends(c) {
                    return Err(e);
                }
                TList::new(Cls::TokenList)
            }
        },
    };
    if let Some(l) = leader {
        token.prepend(l);
    }
    local_part.push(token);
    if !value.is_empty() && (first(&value) == Some('\\') || !starts(&value, phrase_ends)) {
        let joined = local_part.to_str()? + &value;
        let (obs, rest) = get_obs_local_part(&joined)?;
        value = rest;
        if obs.ttype == "invalid-obs-local-part" {
            local_part.defects.push(Defect::Invalid);
        } else {
            local_part.defects.push(Defect::Other);
        }
        local_part.items[0] = obs.into();
    }
    // local_part.value.encode('ascii') -> NonASCIILocalPartDefect (not Invalid); value may raise.
    let _ = local_part.value()?;
    Ok((local_part, value))
}

fn get_obs_local_part(value: &str) -> R<(TList, String)> {
    let mut obs = TList::new(Cls::ObsLocalPart);
    let mut value = value.to_string();
    let mut last_non_ws_was_dot = false;
    while !value.is_empty() && (first(&value) == Some('\\') || !starts(&value, phrase_ends)) {
        if first(&value) == Some('.') {
            if last_non_ws_was_dot {
                obs.defects.push(Defect::Invalid);
            }
            obs.push(dot());
            last_non_ws_was_dot = true;
            value = rest1(&value);
            continue;
        } else if first(&value) == Some('\\') {
            obs.push(vt("\\", "misplaced-special"));
            value = rest1(&value);
            obs.defects.push(Defect::Invalid);
            last_non_ws_was_dot = false;
            continue;
        }
        if !obs.items.is_empty() && obs.items.last().unwrap().ttype()? != "dot" {
            obs.defects.push(Defect::Invalid);
        }
        match get_word(&value) {
            Ok((t, rest)) => {
                last_non_ws_was_dot = false;
                obs.push(t);
                value = rest;
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(e) => {
                if !cfws_leader(ix0(&value)?) {
                    return Err(e);
                }
                let (t, rest) = get_cfws(&value)?;
                obs.push(t);
                value = rest;
            }
        }
    }
    let n = obs.items.len();
    let tt = |i: usize| -> Result<&'static str, PyErr> {
        obs.items
            .get(i)
            .ok_or_else(|| PyErr::new("IndexError", "list index out of range"))?
            .ttype()
    };
    if n == 0 {
        return Err(PErr::Py(PyErr::new(
            "IndexError",
            "list index out of range",
        )));
    }
    if tt(0)? == "dot" || (tt(0)? == "cfws" && tt(1)? == "dot") {
        obs.defects.push(Defect::Invalid);
    }
    if tt(n - 1)? == "dot"
        || (tt(n - 1)? == "cfws" && n >= 2 && tt(n - 2)? == "dot")
        || (tt(n - 1)? == "cfws" && n < 2 && tt(n.wrapping_sub(2))? == "dot")
    {
        obs.defects.push(Defect::Invalid);
    }
    if !obs.defects.is_empty() {
        obs.ttype = "invalid-obs-local-part";
    }
    Ok((obs, value))
}

fn get_dtext(value: &str) -> R<(Term, String)> {
    let (ptext, rest, had_qp) = get_ptext_to_endchars(value, "[]")?;
    let mut t = vt(ptext, "ptext");
    if had_qp {
        t.defects.push(Defect::Other);
    }
    Ok((t, rest))
}

fn check_for_early_dl_end(value: &str, dl: &mut TList) -> bool {
    if !value.is_empty() {
        return false;
    }
    dl.items
        .push(Tok::DefectObj("end of input inside domain-literal".into()));
    dl.push(vt("]", "domain-literal-end"));
    true
}

fn get_domain_literal(value: &str) -> R<(TList, String)> {
    let mut dl = TList::new(Cls::DomainLiteral);
    let mut value = value.to_string();
    if cfws_leader(ix0(&value)?) {
        let (t, rest) = get_cfws(&value)?;
        dl.push(t);
        value = rest;
    }
    if value.is_empty() {
        return hpe("expected domain-literal");
    }
    if first(&value) != Some('[') {
        return hpe("expected '[' at start of domain-literal");
    }
    value = rest1(&value);
    if check_for_early_dl_end(&value, &mut dl) {
        return Ok((dl, value));
    }
    dl.push(vt("[", "domain-literal-start"));
    if starts(&value, wsp) {
        let (t, rest) = get_fws(&value);
        dl.push(t);
        value = rest;
    }
    let (t, rest) = get_dtext(&value)?;
    dl.push(t);
    value = rest;
    if check_for_early_dl_end(&value, &mut dl) {
        return Ok((dl, value));
    }
    if starts(&value, wsp) {
        let (t, rest) = get_fws(&value);
        dl.push(t);
        value = rest;
    }
    if check_for_early_dl_end(&value, &mut dl) {
        return Ok((dl, value));
    }
    if first(&value) != Some(']') {
        return hpe("expected ']' at end of domain-literal");
    }
    dl.push(vt("]", "domain-literal-end"));
    value = rest1(&value);
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        dl.push(t);
        value = rest;
    }
    Ok((dl, value))
}

fn get_domain(value: &str) -> R<(TList, String)> {
    let mut domain = TList::new(Cls::Domain);
    let mut value = value.to_string();
    let mut leader = None;
    if cfws_leader(ix0(&value)?) {
        let (l, rest) = get_cfws(&value)?;
        leader = Some(l);
        value = rest;
    }
    if value.is_empty() {
        return hpe("expected domain");
    }
    if first(&value) == Some('[') {
        let (mut t, rest) = get_domain_literal(&value)?;
        if let Some(l) = leader {
            t.prepend(l);
        }
        domain.push(t);
        return Ok((domain, rest));
    }
    let mut token = match get_dot_atom(&value) {
        Ok((t, rest)) => {
            value = rest;
            t
        }
        Err(PErr::Py(e)) => return Err(PErr::Py(e)),
        Err(_) => {
            let (t, rest) = get_atom(&value)?;
            value = rest;
            t
        }
    };
    if first(&value) == Some('@') {
        return hpe("Invalid Domain");
    }
    if let Some(l) = leader {
        token.prepend(l);
    }
    domain.push(token);
    if first(&value) == Some('.') {
        domain.defects.push(Defect::Other);
        if domain.items[0].ttype()? == "dot-atom" {
            let inner = domain.items[0].as_list().expect("dot-atom").items.clone();
            domain.items = inner;
        }
        while first(&value) == Some('.') {
            domain.push(dot());
            let (t, rest) = get_atom(&rest1(&value))?;
            domain.push(t);
            value = rest;
        }
    }
    Ok((domain, value))
}

/// `get_addr_spec(value)`.
pub fn get_addr_spec(value: &str) -> R<(TList, String)> {
    let mut addr_spec = TList::new(Cls::AddrSpec);
    let (t, mut value) = get_local_part(value)?;
    addr_spec.push(t);
    if value.is_empty() || first(&value) != Some('@') {
        addr_spec.defects.push(Defect::Invalid);
        return Ok((addr_spec, value));
    }
    addr_spec.push(vt("@", "address-at-symbol"));
    let (t, rest) = get_domain(&rest1(&value))?;
    addr_spec.push(t);
    value = rest;
    Ok((addr_spec, value))
}

fn get_obs_route(value: &str) -> R<(TList, String)> {
    let mut obs_route = TList::new(Cls::ObsRoute);
    let mut value = value.to_string();
    while !value.is_empty() && (first(&value) == Some(',') || starts(&value, cfws_leader)) {
        if starts(&value, cfws_leader) {
            let (t, rest) = get_cfws(&value)?;
            obs_route.push(t);
            value = rest;
        } else if first(&value) == Some(',') {
            obs_route.push(list_separator());
            value = rest1(&value);
        }
    }
    if value.is_empty() || first(&value) != Some('@') {
        return hpe("expected obs-route domain");
    }
    obs_route.push(vt("@", "route-component-marker"));
    let (t, rest) = get_domain(&rest1(&value))?;
    obs_route.push(t);
    value = rest;
    while first(&value) == Some(',') {
        obs_route.push(list_separator());
        value = rest1(&value);
        if value.is_empty() {
            break;
        }
        if starts(&value, cfws_leader) {
            let (t, rest) = get_cfws(&value)?;
            obs_route.push(t);
            value = rest;
        }
        if ix0(&value)? == '@' {
            obs_route.push(vt("@", "route-component-marker"));
            let (t, rest) = get_domain(&rest1(&value))?;
            obs_route.push(t);
            value = rest;
        }
    }
    if value.is_empty() {
        return hpe("end of header while parsing obs-route");
    }
    if first(&value) != Some(':') {
        return hpe("expected ':' marking end of obs-route");
    }
    obs_route.push(vt(":", "end-of-obs-route-marker"));
    Ok((obs_route, rest1(&value)))
}

fn get_angle_addr(value: &str) -> R<(TList, String)> {
    let mut angle_addr = TList::new(Cls::AngleAddr);
    let mut value = value.to_string();
    if cfws_leader(ix0(&value)?) {
        let (t, rest) = get_cfws(&value)?;
        angle_addr.push(t);
        value = rest;
    }
    if value.is_empty() || first(&value) != Some('<') {
        return hpe("expected angle-addr");
    }
    angle_addr.push(vt("<", "angle-addr-start"));
    value = rest1(&value);
    if ix0(&value)? == '>' {
        angle_addr.push(vt(">", "angle-addr-end"));
        angle_addr.defects.push(Defect::Invalid);
        value = rest1(&value);
        return Ok((angle_addr, value));
    }
    match get_addr_spec(&value) {
        Ok((t, rest)) => {
            angle_addr.push(t);
            value = rest;
        }
        Err(PErr::Py(e)) => return Err(PErr::Py(e)),
        Err(_) => {
            match get_obs_route(&value) {
                Ok((t, rest)) => {
                    angle_addr.defects.push(Defect::Other);
                    angle_addr.push(t);
                    value = rest;
                }
                Err(PErr::Py(e)) => return Err(PErr::Py(e)),
                Err(_) => return hpe("expected addr-spec or obs-route"),
            }
            let (t, rest) = get_addr_spec(&value)?;
            angle_addr.push(t);
            value = rest;
        }
    }
    if first(&value) == Some('>') {
        value = rest1(&value);
    } else {
        angle_addr.defects.push(Defect::Invalid);
    }
    angle_addr.push(vt(">", "angle-addr-end"));
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        angle_addr.push(t);
        value = rest;
    }
    Ok((angle_addr, value))
}

fn get_display_name(value: &str) -> R<(TList, String)> {
    let mut dn = TList::new(Cls::DisplayName);
    let (token, rest) = get_phrase(value)?;
    dn.items.extend(token.items.iter().cloned());
    dn.defects = token.defects.clone();
    Ok((dn, rest))
}

fn get_name_addr(value: &str) -> R<(TList, String)> {
    let mut name_addr = TList::new(Cls::NameAddr);
    let mut value = value.to_string();
    let mut leader: Option<TList> = None;
    if cfws_leader(ix0(&value)?) {
        let (l, rest) = get_cfws(&value)?;
        value = rest;
        if value.is_empty() {
            return hpe("expected name-addr");
        }
        leader = Some(l);
    }
    if first(&value) != Some('<') {
        if starts(&value, phrase_ends) {
            return hpe("expected name-addr");
        }
        let (mut token, rest) = get_display_name(&value)?;
        value = rest;
        if value.is_empty() {
            return hpe("expected name-addr");
        }
        if let Some(l) = leader.take() {
            match token.items.get_mut(0) {
                None => {
                    return Err(PErr::Py(PyErr::new(
                        "IndexError",
                        "list index out of range",
                    )))
                }
                Some(t0) => prepend_into(t0, l.into())?,
            }
        }
        name_addr.push(token);
    }
    let (mut token, rest) = get_angle_addr(&value)?;
    value = rest;
    if let Some(l) = leader {
        token.prepend(l);
    }
    name_addr.push(token);
    Ok((name_addr, value))
}

fn get_mailbox(value: &str) -> R<(TList, String)> {
    let mut mailbox = TList::new(Cls::Mailbox);
    let (token, value) = match get_name_addr(value) {
        Ok(r) => r,
        Err(PErr::Py(e)) => return Err(PErr::Py(e)),
        Err(_) => match get_addr_spec(value) {
            Ok(r) => r,
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => return hpe("expected mailbox"),
        },
    };
    if token.all_defects()?.contains(&Defect::Invalid) {
        mailbox.ttype = "invalid-mailbox";
    }
    mailbox.push(token);
    Ok((mailbox, value))
}

fn get_invalid_mailbox(value: &str, endchars: &str) -> R<(TList, String)> {
    let mut im = TList::new(Cls::InvalidMailbox);
    let mut value = value.to_string();
    while !value.is_empty() && !endchars.contains(first(&value).unwrap()) {
        if starts(&value, phrase_ends) {
            im.push(vt(first(&value).unwrap().to_string(), "misplaced-special"));
            value = rest1(&value);
        } else {
            let (t, rest) = get_phrase(&value)?;
            im.push(t);
            value = rest;
        }
    }
    Ok((im, value))
}

fn get_mailbox_list(value: &str) -> R<(TList, String)> {
    let mut ml = TList::new(Cls::MailboxList);
    let mut value = value.to_string();
    while !value.is_empty() && first(&value) != Some(';') {
        match get_mailbox(&value) {
            Ok((t, rest)) => {
                ml.push(t);
                value = rest;
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => {
                let mut leader: Option<TList> = None;
                if cfws_leader(ix0(&value)?) {
                    let (l, rest) = get_cfws(&value)?;
                    value = rest;
                    if value.is_empty() || ",;".contains(first(&value).unwrap()) {
                        ml.push(l);
                        ml.defects.push(Defect::Other);
                    } else {
                        let (mut t, rest) = get_invalid_mailbox(&value, ",;")?;
                        value = rest;
                        t.prepend(l);
                        ml.push(t);
                        ml.defects.push(Defect::Invalid);
                    }
                } else if first(&value) == Some(',') {
                    ml.defects.push(Defect::Other);
                } else {
                    let (mut t, rest) = get_invalid_mailbox(&value, ",;")?;
                    value = rest;
                    if let Some(l) = leader.take() {
                        t.prepend(l);
                    }
                    ml.push(t);
                    ml.defects.push(Defect::Invalid);
                }
            }
        }
        if !value.is_empty() && !",;".contains(first(&value).unwrap()) {
            let (t, rest) = get_invalid_mailbox(&value, ",;")?;
            value = rest;
            let last = ml
                .items
                .last_mut()
                .ok_or_else(|| PErr::Py(PyErr::new("IndexError", "list index out of range")))?;
            match last {
                Tok::L(l) => {
                    l.ttype = "invalid-mailbox";
                    l.items.extend(t.items);
                }
                _ => return Err(PErr::Py(PyErr::attr("ValueTerminal", "extend"))),
            }
            ml.defects.push(Defect::Invalid);
        }
        if first(&value) == Some(',') {
            ml.push(list_separator());
            value = rest1(&value);
        }
    }
    Ok((ml, value))
}

fn get_group_list(value: &str) -> R<(TList, String)> {
    let mut gl = TList::new(Cls::GroupList);
    let mut value = value.to_string();
    if value.is_empty() {
        gl.defects.push(Defect::Invalid);
        return Ok((gl, value));
    }
    let mut leader: Option<TList> = None;
    if starts(&value, cfws_leader) {
        let (l, rest) = get_cfws(&value)?;
        value = rest;
        if value.is_empty() {
            gl.defects.push(Defect::Invalid);
            gl.push(l);
            return Ok((gl, value));
        }
        if first(&value) == Some(';') {
            gl.push(l);
            return Ok((gl, value));
        }
        leader = Some(l);
    }
    let (mut token, rest) = get_mailbox_list(&value)?;
    value = rest;
    if token.all_mailboxes()?.is_empty() {
        if let Some(l) = leader {
            gl.push(l);
        }
        gl.items.extend(token.items);
        gl.defects.push(Defect::Other);
        return Ok((gl, value));
    }
    if let Some(l) = leader {
        token.prepend(l);
    }
    gl.push(token);
    Ok((gl, value))
}

fn get_group(value: &str) -> R<(TList, String)> {
    let mut group = TList::new(Cls::Group);
    let (token, mut value) = get_display_name(value)?;
    if value.is_empty() || first(&value) != Some(':') {
        return hpe("expected ':' at end of group display name");
    }
    group.push(token);
    group.push(vt(":", "group-display-name-terminator"));
    value = rest1(&value);
    if first(&value) == Some(';') {
        group.push(vt(";", "group-terminator"));
        return Ok((group, rest1(&value)));
    }
    let (token, rest) = get_group_list(&value)?;
    value = rest;
    group.push(token);
    if value.is_empty() {
        group.defects.push(Defect::Invalid);
    } else if first(&value) != Some(';') {
        return hpe("expected ';' at end of group");
    }
    group.push(vt(";", "group-terminator"));
    value = rest1(&value);
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        group.push(t);
        value = rest;
    }
    Ok((group, value))
}

fn get_address(value: &str) -> R<(TList, String)> {
    let mut address = TList::new(Cls::Address);
    let (token, value) = match get_group(value) {
        Ok(r) => r,
        Err(PErr::Py(e)) => return Err(PErr::Py(e)),
        Err(_) => match get_mailbox(value) {
            Ok(r) => r,
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => return hpe("expected address"),
        },
    };
    address.push(token);
    Ok((address, value))
}

/// `get_address_list(value)`; the remainder is always empty.
pub fn get_address_list(value: &str) -> R<TList> {
    let mut al = TList::new(Cls::AddressList);
    let mut value = value.to_string();
    while !value.is_empty() {
        match get_address(&value) {
            Ok((t, rest)) => {
                al.push(t);
                value = rest;
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => {
                if cfws_leader(ix0(&value)?) {
                    let (l, rest) = get_cfws(&value)?;
                    value = rest;
                    if value.is_empty() || first(&value) == Some(',') {
                        al.push(l);
                        al.defects.push(Defect::Other);
                    } else {
                        let (mut t, rest) = get_invalid_mailbox(&value, ",")?;
                        value = rest;
                        t.prepend(l);
                        al.push(TList::with(Cls::Address, vec![t.into()]));
                        al.defects.push(Defect::Invalid);
                    }
                } else if first(&value) == Some(',') {
                    al.defects.push(Defect::Other);
                } else {
                    let (t, rest) = get_invalid_mailbox(&value, ",")?;
                    value = rest;
                    al.push(TList::with(Cls::Address, vec![t.into()]));
                    al.defects.push(Defect::Invalid);
                }
            }
        }
        if !value.is_empty() && first(&value) != Some(',') {
            let (t, rest) = get_invalid_mailbox(&value, ",")?;
            value = rest;
            let last = al
                .items
                .last_mut()
                .ok_or_else(|| PErr::Py(PyErr::new("IndexError", "list index out of range")))?;
            let mb = match last {
                Tok::L(l) => l
                    .items
                    .get_mut(0)
                    .ok_or_else(|| PErr::Py(PyErr::new("IndexError", "list index out of range")))?,
                Tok::T(_) => return Err(PErr::Py(PyErr::index())),
                Tok::DefectObj(_) => {
                    return Err(PErr::Py(PyErr::new("TypeError", "not subscriptable")))
                }
            };
            match mb {
                Tok::L(l) => {
                    l.ttype = "invalid-mailbox";
                    l.items.extend(t.items);
                }
                _ => return Err(PErr::Py(PyErr::attr("ValueTerminal", "extend"))),
            }
            al.defects.push(Defect::Invalid);
        }
        if !value.is_empty() {
            al.push(list_separator());
            value = rest1(&value);
        }
    }
    Ok(al)
}

fn get_no_fold_literal(value: &str) -> R<(TList, String)> {
    let mut nfl = TList::new(Cls::NoFoldLiteral);
    if value.is_empty() {
        return hpe("expected no-fold-literal");
    }
    if first(value) != Some('[') {
        return hpe("expected '[' at the start of no-fold-literal");
    }
    nfl.push(vt("[", "no-fold-literal-start"));
    let value = rest1(value);
    let (t, value) = get_dtext(&value)?;
    nfl.push(t);
    if value.is_empty() || first(&value) != Some(']') {
        return hpe("expected ']' at the end of no-fold-literal");
    }
    nfl.push(vt("]", "no-fold-literal-end"));
    Ok((nfl, rest1(&value)))
}

fn get_msg_id(value: &str) -> R<(TList, String)> {
    let mut msg_id = TList::new(Cls::MsgId);
    let mut value = value.to_string();
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        msg_id.push(t);
        value = rest;
    }
    if value.is_empty() || first(&value) != Some('<') {
        return hpe("expected msg-id");
    }
    msg_id.push(vt("<", "msg-id-start"));
    value = rest1(&value);
    match get_dot_atom_text(&value) {
        Ok((t, rest)) => {
            msg_id.push(t);
            value = rest;
        }
        Err(PErr::Py(e)) => return Err(PErr::Py(e)),
        Err(_) => match get_obs_local_part(&value) {
            Ok((t, rest)) => {
                msg_id.defects.push(Defect::Other);
                msg_id.push(t);
                value = rest;
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => return hpe("expected dot-atom-text or obs-id-left"),
        },
    }
    if value.is_empty() || first(&value) != Some('@') {
        msg_id.defects.push(Defect::Invalid);
        if first(&value) == Some('>') {
            msg_id.push(vt(">", "msg-id-end"));
            value = rest1(&value);
        }
        return Ok((msg_id, value));
    }
    msg_id.push(vt("@", "address-at-symbol"));
    value = rest1(&value);
    match get_dot_atom_text(&value) {
        Ok((t, rest)) => {
            msg_id.push(t);
            value = rest;
        }
        Err(PErr::Py(e)) => return Err(PErr::Py(e)),
        Err(_) => match get_no_fold_literal(&value) {
            Ok((t, rest)) => {
                msg_id.push(t);
                value = rest;
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => match get_domain(&value) {
                Ok((t, rest)) => {
                    msg_id.defects.push(Defect::Other);
                    msg_id.push(t);
                    value = rest;
                }
                Err(PErr::Py(e)) => return Err(PErr::Py(e)),
                Err(_) => return hpe("expected dot-atom-text, no-fold-literal or obs-id-right"),
            },
        },
    }
    if first(&value) == Some('>') {
        value = rest1(&value);
    } else {
        msg_id.defects.push(Defect::Invalid);
    }
    msg_id.push(vt(">", "msg-id-end"));
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        msg_id.push(t);
        value = rest;
    }
    Ok((msg_id, value))
}

/// `parse_message_id(value)`.
pub fn parse_message_id(value: &str) -> Result<TList, PyErr> {
    let mut message_id = TList::new(Cls::MessageId);
    match get_msg_id(value) {
        Ok((t, rest)) => {
            message_id.push(t);
            if !rest.is_empty() {
                message_id.defects.push(Defect::Invalid);
            }
            Ok(message_id)
        }
        Err(PErr::Py(e)) => Err(e),
        Err(_) => {
            let token = get_unstructured(value);
            let mut inv = TList::with(Cls::InvalidMessageId, token.items);
            inv.defects.push(Defect::Invalid);
            Ok(inv)
        }
    }
}

/// `parse_mime_version(value)`.
pub fn parse_mime_version(value: &str) -> Result<TList, PyErr> {
    let mut mv = TList::new(Cls::MimeVersion);
    let r = (|| -> R<()> {
        if value.is_empty() {
            mv.defects.push(Defect::Other);
            return Ok(());
        }
        let mut value = value.to_string();
        if starts(&value, cfws_leader) {
            let (t, rest) = get_cfws(&value)?;
            mv.push(t);
            value = rest;
            if value.is_empty() {
                mv.defects.push(Defect::Other);
            }
        }
        let mut digits = String::new();
        while !value.is_empty() && first(&value) != Some('.') && !starts(&value, cfws_leader) {
            digits.push(first(&value).unwrap());
            value = rest1(&value);
        }
        let mut major = None;
        if !is_digit_str(&digits) {
            mv.defects.push(Defect::Invalid);
            mv.push(vt(digits.clone(), "xtext"));
        } else {
            major = Some(
                py_int(&digits)
                    .ok_or_else(|| PyErr::new("ValueError", "invalid literal for int()"))?,
            );
            mv.push(vt(digits.clone(), "digits"));
        }
        if starts(&value, cfws_leader) {
            let (t, rest) = get_cfws(&value)?;
            mv.push(t);
            value = rest;
        }
        if value.is_empty() || first(&value) != Some('.') {
            if major.is_some() {
                mv.defects.push(Defect::Invalid);
            }
            if !value.is_empty() {
                mv.push(vt(value.clone(), "xtext"));
            }
            return Ok(());
        }
        mv.push(vt(".", "version-separator"));
        value = rest1(&value);
        if starts(&value, cfws_leader) {
            let (t, rest) = get_cfws(&value)?;
            mv.push(t);
            value = rest;
        }
        if value.is_empty() {
            if major.is_some() {
                mv.defects.push(Defect::Invalid);
            }
            return Ok(());
        }
        let mut digits = String::new();
        while !value.is_empty() && !starts(&value, cfws_leader) {
            digits.push(first(&value).unwrap());
            value = rest1(&value);
        }
        if !is_digit_str(&digits) {
            mv.defects.push(Defect::Invalid);
            mv.push(vt(digits, "xtext"));
        } else {
            py_int(&digits).ok_or_else(|| PyErr::new("ValueError", "invalid literal for int()"))?;
            mv.push(vt(digits, "digits"));
        }
        if starts(&value, cfws_leader) {
            let (t, rest) = get_cfws(&value)?;
            mv.push(t);
            value = rest;
        }
        if !value.is_empty() {
            mv.defects.push(Defect::Invalid);
            mv.push(vt(value, "xtext"));
        }
        Ok(())
    })();
    match r {
        Ok(()) => Ok(mv),
        Err(PErr::Py(e)) => Err(e),
        Err(PErr::Hpe(m)) | Err(PErr::Invalid(m)) => Err(PyErr::new("HeaderParseError", m)),
    }
}

fn get_invalid_parameter(value: &str) -> R<(TList, String)> {
    let mut ip = TList::new(Cls::InvalidParameter);
    let mut value = value.to_string();
    while !value.is_empty() && first(&value) != Some(';') {
        if starts(&value, phrase_ends) {
            ip.push(vt(first(&value).unwrap().to_string(), "misplaced-special"));
            value = rest1(&value);
        } else {
            let (t, rest) = get_phrase(&value)?;
            ip.push(t);
            value = rest;
        }
    }
    Ok((ip, value))
}

fn get_ttext(value: &str) -> R<(Term, String)> {
    let end = value.find(token_ends).unwrap_or(value.len());
    if end == 0 {
        return hpe("expected ttext");
    }
    Ok((vt(&value[..end], "ttext"), value[end..].to_string()))
}

fn get_token(value: &str) -> R<(TList, String)> {
    let mut mtoken = TList::new(Cls::Token);
    let mut value = value.to_string();
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        mtoken.push(t);
        value = rest;
    }
    if starts(&value, token_ends) {
        return hpe("expected token");
    }
    let (t, rest) = get_ttext(&value)?;
    mtoken.push(t);
    value = rest;
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        mtoken.push(t);
        value = rest;
    }
    Ok((mtoken, value))
}

fn get_attrtext(value: &str) -> R<(Term, String)> {
    let end = value.find(attribute_ends).unwrap_or(value.len());
    if end == 0 {
        return hpe("expected attrtext");
    }
    Ok((vt(&value[..end], "attrtext"), value[end..].to_string()))
}

fn get_attribute(value: &str) -> R<(TList, String)> {
    let mut attribute = TList::new(Cls::Attribute);
    let mut value = value.to_string();
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        attribute.push(t);
        value = rest;
    }
    if starts(&value, attribute_ends) {
        return hpe("expected token");
    }
    let (t, rest) = get_attrtext(&value)?;
    attribute.push(t);
    value = rest;
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        attribute.push(t);
        value = rest;
    }
    Ok((attribute, value))
}

fn get_extended_attrtext(value: &str) -> R<(Term, String)> {
    let end = value.find(extended_attribute_ends).unwrap_or(value.len());
    if end == 0 {
        return hpe("expected extended attrtext");
    }
    Ok((
        vt(&value[..end], "extended-attrtext"),
        value[end..].to_string(),
    ))
}

fn get_extended_attribute(value: &str) -> R<(TList, String)> {
    let mut attribute = TList::new(Cls::Attribute);
    let mut value = value.to_string();
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        attribute.push(t);
        value = rest;
    }
    if starts(&value, extended_attribute_ends) {
        return hpe("expected token");
    }
    let (t, rest) = get_extended_attrtext(&value)?;
    attribute.push(t);
    value = rest;
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        attribute.push(t);
        value = rest;
    }
    Ok((attribute, value))
}

fn get_section(value: &str) -> R<(TList, String)> {
    let mut section = TList::new(Cls::Section);
    if value.is_empty() || first(value) != Some('*') {
        return hpe("Expected section");
    }
    section.push(vt("*", "section-marker"));
    let mut value = rest1(value);
    if value.is_empty() || !ps::is_digit(first(&value).unwrap()) {
        return hpe("Expected section number");
    }
    let mut digits = String::new();
    while !value.is_empty() && ps::is_digit(first(&value).unwrap()) {
        digits.push(first(&value).unwrap());
        value = rest1(&value);
    }
    if digits.starts_with('0') && digits != "0" {
        section.defects.push(Defect::Invalid);
    }
    section.number = Some(py_int(&digits).ok_or_else(|| {
        PErr::Py(PyErr::new(
            "ValueError",
            "invalid literal for int() with base 10",
        ))
    })?);
    section.push(vt(digits, "digits"));
    Ok((section, value))
}

fn get_value(value: &str) -> R<(TList, String)> {
    let mut v = TList::new(Cls::Value);
    if value.is_empty() {
        return hpe("Expected value but found end of string");
    }
    let mut value = value.to_string();
    let mut leader = None;
    if starts(&value, cfws_leader) {
        let (l, rest) = get_cfws(&value)?;
        leader = Some(l);
        value = rest;
    }
    if value.is_empty() {
        return hpe("Expected value but found only cfws");
    }
    let (mut token, rest) = if first(&value) == Some('"') {
        get_quoted_string(&value)?
    } else {
        get_extended_attribute(&value)?
    };
    if let Some(l) = leader {
        token.prepend(l);
    }
    v.push(token);
    Ok((v, rest))
}

/// Where `get_parameter` appends: the parameter itself or the emptied bare-quoted-string inside it.
enum AppendTo {
    Param,
    Bqs(usize, usize),
}

fn append_to(param: &mut TList, at: &AppendTo, tok: Tok) {
    match at {
        AppendTo::Param => param.items.push(tok),
        AppendTo::Bqs(qi, bi) => {
            if let Tok::L(qs) = &mut param.items[*qi] {
                if let Tok::L(bqs) = &mut qs.items[*bi] {
                    bqs.items.push(tok);
                }
            }
        }
    }
}

fn get_parameter(value: &str) -> R<(TList, String)> {
    let mut param = TList::new(Cls::Parameter);
    let (token, mut value) = get_attribute(value)?;
    param.push(token);
    if value.is_empty() || first(&value) == Some(';') {
        param.defects.push(Defect::Invalid);
        return Ok((param, value));
    }
    if first(&value) == Some('*') {
        match get_section(&value) {
            Ok((t, rest)) => {
                param.sectioned = true;
                param.push(t);
                value = rest;
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => {}
        }
        if value.is_empty() {
            return hpe("Incomplete parameter");
        }
        if first(&value) == Some('*') {
            param.push(vt("*", "extended-parameter-marker"));
            value = rest1(&value);
            param.extended = true;
        }
    }
    if first(&value) != Some('=') {
        return hpe("Parameter not followed by '='");
    }
    param.push(vt("=", "parameter-separator"));
    value = rest1(&value);
    if starts(&value, cfws_leader) {
        let (t, rest) = get_cfws(&value)?;
        param.push(t);
        value = rest;
    }
    let mut remainder: Option<String> = None;
    let mut appendto = AppendTo::Param;
    if param.extended && first(&value) == Some('"') {
        let (qstring, rem) = get_quoted_string(&value)?;
        remainder = Some(rem);
        let inner_value = qstring.quoted_stripped_value()?.unwrap_or_default();
        let mut semi_valid = false;
        if param.section_number()? == 0 {
            if first(&inner_value) == Some('\'') {
                semi_valid = true;
            } else {
                let (_t, rest) = get_attrtext(&inner_value)?;
                if first(&rest) == Some('\'') {
                    semi_valid = true;
                }
            }
        } else {
            // A bare `except:` swallows anything here.
            if let Ok((_t, rest)) = get_extended_attrtext(&inner_value) {
                if rest.is_empty() {
                    semi_valid = true;
                }
            }
        }
        if semi_valid {
            param.defects.push(Defect::Invalid);
            let qi = param.items.len();
            param.push(qstring);
            let mut found = None;
            if let Tok::L(qs) = &mut param.items[qi] {
                for (bi, t) in qs.items.iter_mut().enumerate() {
                    if t.ttype()? == "bare-quoted-string" {
                        if let Tok::L(b) = t {
                            b.items.clear();
                        }
                        found = Some(bi);
                        break;
                    }
                }
            }
            if let Some(bi) = found {
                appendto = AppendTo::Bqs(qi, bi);
            }
            value = inner_value;
        } else {
            remainder = None;
            param.defects.push(Defect::Invalid);
        }
    }
    let mut token: Option<TList> = None;
    if first(&value) != Some('\'') {
        let (t, rest) = get_value(&value)?;
        token = Some(t);
        value = rest;
    }
    if !param.extended || param.section_number()? > 0 {
        if value.is_empty() || first(&value) != Some('\'') {
            if let Some(t) = token {
                append_to(&mut param, &appendto, t.into());
            } else {
                // appendto.append(None): a None in the tree; str() of it would fail later.
                return Err(PErr::Py(PyErr::new("TypeError", "None in token list")));
            }
            if let Some(rem) = remainder {
                value = rem;
            }
            return Ok((param, value));
        }
        param.defects.push(Defect::Invalid);
    }
    if value.is_empty() {
        param.defects.push(Defect::Invalid);
        match token {
            Some(t) => append_to(&mut param, &appendto, t.into()),
            None => return Err(PErr::Py(PyErr::new("TypeError", "None in token list"))),
        }
        if remainder.is_none() {
            return Ok((param, value));
        }
    } else {
        if let Some(tok) = token {
            // for t in token: if t.token_type == 'extended-attrtext': break  (t is the last item otherwise)
            let mut chosen: Option<Tok> = None;
            for t in &tok.items {
                chosen = Some(t.clone());
                if t.ttype()? == "extended-attrtext" {
                    break;
                }
            }
            let t = chosen
                .ok_or_else(|| PErr::Py(PyErr::new("NameError", "name 't' is not defined")))?;
            param.charset = t.value()?;
            append_to(&mut param, &appendto, t);
        }
        if first(&value) != Some('\'') {
            return hpe("Expected RFC2231 char/lang encoding delimiter");
        }
        append_to(&mut param, &appendto, vt("'", "RFC2231-delimiter").into());
        value = rest1(&value);
        if !value.is_empty() && first(&value) != Some('\'') {
            let (t, rest) = get_attrtext(&value)?;
            value = rest;
            append_to(&mut param, &appendto, t.into());
            if value.is_empty() || first(&value) != Some('\'') {
                return hpe("Expected RFC2231 char/lang encoding delimiter");
            }
        }
        append_to(&mut param, &appendto, vt("'", "RFC2231-delimiter").into());
        value = rest1(&value);
    }
    let token: TList = if remainder.is_some() {
        let mut v = TList::new(Cls::Value);
        while !value.is_empty() {
            if starts(&value, wsp) {
                let (t, rest) = get_fws(&value);
                v.push(t);
                value = rest;
            } else if first(&value) == Some('"') {
                v.push(vt("\"", "DQUOTE"));
                value = rest1(&value);
            } else {
                let (t, rest) = get_qcontent(&value)?;
                v.push(t);
                value = rest;
            }
        }
        v
    } else {
        let (t, rest) = get_value(&value)?;
        value = rest;
        t
    };
    append_to(&mut param, &appendto, token.into());
    if let Some(rem) = remainder {
        value = rem;
    }
    Ok((param, value))
}

/// `parse_mime_parameters(value)`.
fn parse_mime_parameters(value: &str) -> R<TList> {
    let mut mp = TList::new(Cls::MimeParameters);
    let mut value = value.to_string();
    while !value.is_empty() {
        match get_parameter(&value) {
            Ok((t, rest)) => {
                mp.push(t);
                value = rest;
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => {
                let mut leader: Option<TList> = None;
                if cfws_leader(ix0(&value)?) {
                    let (l, rest) = get_cfws(&value)?;
                    leader = Some(l);
                    value = rest;
                }
                if value.is_empty() {
                    // mime_parameters.append(leader): leader is a CFWSList here.
                    if let Some(l) = leader {
                        mp.push(l);
                    } else {
                        return Err(PErr::Py(PyErr::new("TypeError", "None in token list")));
                    }
                    return Ok(mp);
                }
                if first(&value) == Some(';') {
                    if let Some(l) = leader {
                        mp.push(l);
                    }
                    mp.defects.push(Defect::Invalid);
                } else {
                    let (mut t, rest) = get_invalid_parameter(&value)?;
                    value = rest;
                    if let Some(l) = leader {
                        if !l.items.is_empty() {
                            t.prepend(l);
                        }
                    }
                    mp.push(t);
                    mp.defects.push(Defect::Invalid);
                }
            }
        }
        if !value.is_empty() && first(&value) != Some(';') {
            let (t, rest) = get_invalid_parameter(&value)?;
            value = rest;
            match mp.items.last_mut() {
                Some(Tok::L(l)) => {
                    l.ttype = "invalid-parameter";
                    l.items.extend(t.items);
                }
                _ => return Err(PErr::Py(PyErr::attr("ValueTerminal", "extend"))),
            }
            mp.defects.push(Defect::Invalid);
        }
        if !value.is_empty() {
            mp.push(vt(";", "parameter-separator"));
            value = rest1(&value);
        }
    }
    Ok(mp)
}

fn find_mime_parameters(tokenlist: &mut TList, value: &str) -> R<()> {
    let mut value = value.to_string();
    while !value.is_empty() && first(&value) != Some(';') {
        if starts(&value, phrase_ends) {
            tokenlist.push(vt(first(&value).unwrap().to_string(), "misplaced-special"));
            value = rest1(&value);
        } else {
            let (t, rest) = get_phrase(&value)?;
            tokenlist.push(t);
            value = rest;
        }
    }
    if value.is_empty() {
        return Ok(());
    }
    tokenlist.push(vt(";", "parameter-separator"));
    let mp = parse_mime_parameters(&rest1(&value))?;
    tokenlist.push(mp);
    Ok(())
}

fn finish(r: R<TList>) -> Result<TList, PyErr> {
    match r {
        Ok(t) => Ok(t),
        Err(PErr::Py(e)) => Err(e),
        Err(PErr::Hpe(m)) | Err(PErr::Invalid(m)) => Err(PyErr::new("HeaderParseError", m)),
    }
}

/// `parse_content_type_header(value)`.
pub fn parse_content_type_header(value: &str) -> Result<TList, PyErr> {
    finish((|| {
        let mut ctype = TList::new(Cls::ContentType);
        if value.is_empty() {
            ctype.defects.push(Defect::Other);
            return Ok(ctype);
        }
        let (token, mut value) = match get_token(value) {
            Ok(r) => r,
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => {
                ctype.defects.push(Defect::Invalid);
                find_mime_parameters(&mut ctype, value)?;
                return Ok(ctype);
            }
        };
        ctype.push(token);
        if value.is_empty() || first(&value) != Some('/') {
            ctype.defects.push(Defect::Invalid);
            if !value.is_empty() {
                find_mime_parameters(&mut ctype, &value)?;
            }
            return Ok(ctype);
        }
        ctype.push(vt("/", "content-type-separator"));
        value = rest1(&value);
        let (token, rest) = match get_token(&value) {
            Ok(r) => r,
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => {
                ctype.defects.push(Defect::Invalid);
                find_mime_parameters(&mut ctype, &value)?;
                return Ok(ctype);
            }
        };
        ctype.push(token);
        value = rest;
        if value.is_empty() {
            return Ok(ctype);
        }
        if first(&value) != Some(';') {
            ctype.defects.push(Defect::Invalid);
            find_mime_parameters(&mut ctype, &value)?;
            return Ok(ctype);
        }
        ctype.push(vt(";", "parameter-separator"));
        let mp = parse_mime_parameters(&rest1(&value))?;
        ctype.push(mp);
        Ok(ctype)
    })())
}

/// `parse_content_disposition_header(value)`.
pub fn parse_content_disposition_header(value: &str) -> Result<TList, PyErr> {
    finish((|| {
        let mut disp = TList::new(Cls::ContentDisposition);
        if value.is_empty() {
            disp.defects.push(Defect::Other);
            return Ok(disp);
        }
        let (token, value) = match get_token(value) {
            Ok(r) => r,
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => {
                disp.defects.push(Defect::Invalid);
                find_mime_parameters(&mut disp, value)?;
                return Ok(disp);
            }
        };
        disp.push(token);
        if value.is_empty() {
            return Ok(disp);
        }
        if first(&value) != Some(';') {
            disp.defects.push(Defect::Invalid);
            find_mime_parameters(&mut disp, &value)?;
            return Ok(disp);
        }
        disp.push(vt(";", "parameter-separator"));
        let mp = parse_mime_parameters(&rest1(&value))?;
        disp.push(mp);
        Ok(disp)
    })())
}

/// `parse_content_transfer_encoding_header(value)`.
pub fn parse_content_transfer_encoding_header(value: &str) -> Result<TList, PyErr> {
    finish((|| {
        let mut cte = TList::new(Cls::ContentTransferEncoding);
        if value.is_empty() {
            cte.defects.push(Defect::Other);
            return Ok(cte);
        }
        let mut value = value.to_string();
        match get_token(&value) {
            Ok((t, rest)) => {
                cte.push(t);
                value = rest;
            }
            Err(PErr::Py(e)) => return Err(PErr::Py(e)),
            Err(_) => cte.defects.push(Defect::Invalid),
        }
        while !value.is_empty() {
            cte.defects.push(Defect::Invalid);
            if starts(&value, phrase_ends) {
                cte.push(vt(first(&value).unwrap().to_string(), "misplaced-special"));
                value = rest1(&value);
            } else {
                let (t, rest) = get_phrase(&value)?;
                cte.push(t);
                value = rest;
            }
        }
        Ok(cte)
    })())
}

/// Used by the address helpers to know whether a str has escaped bytes.
pub fn has_escaped(s: &str) -> bool {
    has_surrogates(s)
}

/// `bytes.decode('ascii', 'surrogateescape')` re-export for header users.
pub fn escaped(data: &[u8]) -> String {
    decode_ascii_escaped(data)
}

// `DOT_ATOM_ENDS` for headerregistry.Address.addr_spec.
pub fn is_dot_atom_end(c: char) -> bool {
    dot_atom_ends(c)
}

/// `SPECIALS` membership for headerregistry.Address.__str__.
pub fn is_special(c: char) -> bool {
    specials(c)
}
