//! `email.headerregistry` with `policy.default`: the string value (`str(header)`) a header object has.

use super::headervalue::{self as hvp, PyErr, TList};
use super::pystr::{has_surrogates, lower, sanitize};
use super::utils;

/// `headerregistry.Address(display_name, username, domain)` as a string.
struct Address {
    display_name: String,
    username: String,
    domain: String,
}

impl Address {
    fn new(display_name: String, username: String, domain: String) -> Result<Address, PyErr> {
        let inputs = format!("{display_name}{username}{domain}");
        if inputs.contains('\r') || inputs.contains('\n') {
            return Err(PyErr::new(
                "ValueError",
                "invalid arguments; address parts cannot contain CR or LF",
            ));
        }
        Ok(Address {
            display_name,
            username,
            domain,
        })
    }

    fn addr_spec(&self) -> String {
        let mut lp = self.username.clone();
        if lp.chars().any(hvp::is_dot_atom_end) {
            lp = hvp::quote_string(&lp);
        }
        if !self.domain.is_empty() {
            return format!("{}@{}", lp, self.domain);
        }
        if lp.is_empty() {
            return "<>".into();
        }
        lp
    }

    fn to_str(&self) -> String {
        let mut disp = self.display_name.clone();
        if disp.chars().any(hvp::is_special) {
            disp = hvp::quote_string(&disp);
        }
        if !disp.is_empty() {
            let spec = self.addr_spec();
            let spec = if spec == "<>" { String::new() } else { spec };
            return format!("{} <{}>", disp, spec);
        }
        self.addr_spec()
    }
}

struct Group {
    display_name: Option<String>,
    addresses: Vec<Address>,
}

impl Group {
    fn to_str(&self) -> String {
        if self.display_name.is_none() && self.addresses.len() == 1 {
            return self.addresses[0].to_str();
        }
        let disp = match &self.display_name {
            None => "None".to_string(),
            Some(d) => {
                if d.chars().any(hvp::is_special) {
                    hvp::quote_string(d)
                } else {
                    d.clone()
                }
            }
        };
        let adr: Vec<String> = self.addresses.iter().map(|a| a.to_str()).collect();
        let adrstr = adr.join(", ");
        let adrstr = if adrstr.is_empty() {
            adrstr
        } else {
            format!(" {adrstr}")
        };
        format!("{}:{};", disp, adrstr)
    }
}

/// One address of an address header: (display name, username, domain) as headerregistry keeps them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedAddress {
    pub display_name: String,
    pub username: String,
    pub domain: String,
}

fn address_groups(value: &str) -> Result<Vec<Group>, PyErr> {
    let address_list: TList = match hvp::get_address_list(value) {
        Ok(t) => t,
        Err(hvp::PErr::Py(e)) => return Err(e),
        Err(hvp::PErr::Hpe(m)) | Err(hvp::PErr::Invalid(m)) => {
            return Err(PyErr::new("HeaderParseError", m))
        }
    };
    let mut groups = Vec::new();
    for addr in address_list.addresses()? {
        let display = addr.display_name()?;
        let mut addresses = Vec::new();
        for mb in addr.all_mailboxes()? {
            let dn = mb.display_name()?.unwrap_or_default();
            let lp = mb.local_part()?.unwrap_or_default();
            let dom = mb.domain()?.unwrap_or_default();
            addresses.push(Address::new(dn, lp, dom)?);
        }
        groups.push(Group {
            display_name: display,
            addresses,
        });
    }
    // defects = list(address_list.all_defects): evaluated (and may raise) even though unused.
    address_list.all_defects()?;
    Ok(groups)
}

/// The addresses of an address header (`header.addresses`), for callers that want them structured.
pub fn parse_addresses(value: &str) -> Result<Vec<ParsedAddress>, PyErr> {
    Ok(address_groups(value)?
        .into_iter()
        .flat_map(|g| g.addresses)
        .map(|a| ParsedAddress {
            display_name: a.display_name,
            username: a.username,
            domain: a.domain,
        })
        .collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Unstructured,
    Date,
    Address,
    MimeVersion,
    ContentType,
    ContentDisposition,
    Cte,
    MessageId,
}

fn kind(name: &str) -> Kind {
    match lower(name).as_str() {
        "date" | "resent-date" | "orig-date" => Kind::Date,
        "sender" | "resent-sender" | "to" | "resent-to" | "cc" | "resent-cc" | "bcc"
        | "resent-bcc" | "from" | "resent-from" | "reply-to" => Kind::Address,
        "mime-version" => Kind::MimeVersion,
        "content-type" => Kind::ContentType,
        "content-disposition" => Kind::ContentDisposition,
        "content-transfer-encoding" => Kind::Cte,
        "message-id" => Kind::MessageId,
        _ => Kind::Unstructured,
    }
}

/// `str(policy.header_factory(name, value))`: the decoded header value (value already unfolded).
pub fn decoded(name: &str, value: &str) -> Result<String, PyErr> {
    let d = match kind(name) {
        Kind::Unstructured => hvp::get_unstructured(value).to_str()?,
        Kind::Date => {
            if value.is_empty() {
                String::new()
            } else {
                match utils::parsedate_to_datetime(value) {
                    Ok(dt) => utils::format_datetime(&dt),
                    Err(_) => value.to_string(),
                }
            }
        }
        Kind::Address => {
            let groups = address_groups(value)?;
            groups
                .iter()
                .map(|g| g.to_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
        Kind::MimeVersion => hvp::parse_mime_version(value)?.to_str()?,
        Kind::ContentType => {
            let t = hvp::parse_content_type_header(value)?;
            let s = t.to_str()?;
            // kwds['params'] walks the parameters again (same result; any error was raised above).
            s
        }
        Kind::ContentDisposition => hvp::parse_content_disposition_header(value)?.to_str()?,
        Kind::Cte => hvp::parse_content_transfer_encoding_header(value)?.to_str()?,
        Kind::MessageId => hvp::parse_message_id(value)?.to_str()?,
    };
    Ok(if has_surrogates(&d) { sanitize(&d) } else { d })
}
