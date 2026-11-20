//! How Nary writes to customers, everywhere (Nary fix batch 1, B3).
//!
//! Every code path that writes a customer message (WhatsApp replies, price answers, re-engagement,
//! acknowledgements, quotation follow-ups, the lost-deal reply, the meeting summary email) builds its system
//! prompt with [`customer_prompt`]: the customer register, the Owner's approved style guide for the channel,
//! the fixed writing rules below and the Owner-approved skills (their full procedures: a background job has no
//! use_skill tool, so the steps are in the prompt).
//!
//! Before a customer message becomes a card (and so before any send), [`review_card`] runs the post-write
//! linter: call offers other than Google Meet, em dashes, promises to send a document staff have not confirmed, and
//! (in Nary's own follow-ups) any call or meeting offer, by the patterns of nary.writing.meeting_offer_patterns.
//! On a hit Nary rewrites the message once; when the rewrite still breaks a rule (or changes a number) the
//! original is kept, the problems are written on the card and the card waits for the Owner.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use granary_compat::text::pystr;
use granary_core::errors::ApiResult;
use granary_core::settings_store::{get_optional, SettingError};
use regex::Regex;
use serde_json::{json, Value};
use sqlx::PgConnection;

pub const WHERE: &str = "Nary & AI → Replies & pacing";
/// The on/off setting of the post-write check.
pub const LINT_SETTING: &str = "nary.writing.lint_enabled";
/// The model job of the one rewrite attempt.
pub const FIX_JOB: &str = "nary.style_fix";

/// The rules every customer message follows (Owner corrections, 13-15 Oct 2026). Stable text: part of the
/// cached prompt.
pub const CUSTOMER_WRITING_RULES: &str = "WRITING RULES FOR EVERY CUSTOMER MESSAGE (set by the Owner):
- When you open with a time greeting (Good morning, Good afternoon or Good evening), it is set to the
  time of sending automatically.
- Never offer a call, meeting, demo or video call unless the customer asked for one in this conversation.
  When they ask, offer only a Google Meet video call; never Zoom, Microsoft Teams, a phone call or any other
  tool.
- Never promise to send a document (quotation, proposal, brochure, datasheet, contract, invoice) unless
  your instructions say staff confirmed it; say the team will follow up instead.
- Never use em dashes.";

// ─── The prompt ──────────────────────────────────────────────────────────────

/// The approved skills with their procedures, for a job without the use_skill tool (capped by
/// nary.skills.max_in_prompt, newest first). "" when none is approved.
pub async fn skills_block(db: &mut PgConnection) -> ApiResult<String> {
    super::skills::prompt_procedures(db).await
}

/// The "customer" system prompt every customer-writing path uses: identity, rules, the customer register,
/// the Owner's style guide for `channel`, the writing rules, the approved skills, then the job's own `extra`
/// instructions; `context` (what changes per call) after the cache break.
pub async fn customer_prompt(
    db: &mut PgConnection,
    channel: Option<&str>,
    extra: &str,
    context: &str,
) -> ApiResult<String> {
    let style = super::persona::style_guide(db, channel).await?;
    let skills = skills_block(db).await?;
    Ok(customer_prompt_with(&style, &skills, extra, context))
}

/// [`customer_prompt`] from already loaded parts (no database).
pub fn customer_prompt_with(style: &str, skills: &str, extra: &str, context: &str) -> String {
    let parts: Vec<&str> = [CUSTOMER_WRITING_RULES, skills, extra]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    super::persona::system_prompt("customer", &parts.join("\n\n"), style, context)
}

/// Tests: a customer prompt without the writing rules and the skills block, as it was before they were
/// added (golden replays compare the rest byte for byte).
pub fn without_writing_rules(system: &str) -> String {
    system.replace(&format!("\n\n{CUSTOMER_WRITING_RULES}"), "")
}

// ─── Time greeting ───────────────────────────────────────────────────────────

/// The time greeting for a local hour from the greeting starts of nary.wa.greeting_hours
/// (reply_guard::greeting_hours): "Good morning/afternoon/evening". The same table re-greets a message when
/// it is sent (reply_guard::regreet), so the two never disagree (batch 2 T3). Messages are written in English;
/// `language` is accepted for the callers and does not change the greeting.
pub fn time_greeting(hour: u32, _language: &str, starts: &[(String, u32)]) -> String {
    let word = super::reply_guard::greeting_word(hour, starts);
    format!("Good {}", super::reply_guard::english_of(&word))
}

// ─── The linter ──────────────────────────────────────────────────────────────

/// One broken rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// call_tool, em_dash, document_promise, meeting_offer
    pub code: &'static str,
    pub detail: String,
}

/// What the linter needs to know about a message.
#[derive(Debug, Clone, Copy)]
pub struct LintInput<'a> {
    pub text: &'a str,
    /// "whatsapp" or "email"
    pub channel: &'a str,
    /// the message language when known (messages are written in English)
    pub language: Option<&'a str>,
    /// staff confirmed a document goes with it (an attachment on the card)
    pub document_confirmed: bool,
}

/// "phone number", "mobile number": asking for a number is not offering a call.
static PHONE_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(phone|mobile|cell|whatsapp|telephone)\s+(number|no\.?)\b").unwrap()
});
static CALL_TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(zoom|ms\s+teams|microsoft\s+teams|teams\s+(meeting|call)|skype|webex|whatsapp\s+(call|video)|video\s*call\s+(via|over|on)\s+whatsapp|phone\s+call|call\s+you|give\s+you\s+a\s+call|over\s+the\s+phone|by\s+phone)\b",
    )
    .unwrap()
});
const DOCS: &str = r"(quotation|quote|proposal|brochure|datasheet|data\s*sheet|catalog(ue)?|document|contract|invoice|company\s+profile|presentation|deck)";
static PROMISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(i|we)('ll|\s+will|\s+shall)\s+(send|share|email|forward)\b[^.!?\n]{{0,80}}\b{DOCS}|\b{DOCS}\b[^.!?\n]{{0,60}}\b(will\s+be\s+(sent|shared|emailed|forwarded))"
    ))
    .unwrap()
});
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());

/// The language Nary writes in. Granary writes customer messages in English.
pub fn guess_language(_text: &str) -> &'static str {
    "en"
}

/// The rules a customer message breaks (empty when it is fine).
pub fn lint(input: LintInput<'_>) -> Vec<Finding> {
    let text = input.text;
    let mut out = Vec::new();
    let scrubbed = PHONE_NUMBER.replace_all(text, " ");
    if let Some(m) = CALL_TOOL.find(&scrubbed) {
        out.push(Finding {
            code: "call_tool",
            detail: format!("offers a call other than Google Meet (\"{}\")", m.as_str()),
        });
    }
    if text.contains('\u{2014}') {
        out.push(Finding {
            code: "em_dash",
            detail: "has an em dash".into(),
        });
    }
    if !input.document_confirmed {
        if let Some(m) = PROMISE.find(text) {
            out.push(Finding {
                code: "document_promise",
                detail: format!(
                    "promises a document staff have not confirmed (\"{}\")",
                    super::py_slice(m.as_str(), 80)
                ),
            });
        }
    }
    out
}

/// The findings as one line for a card.
pub fn describe(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| f.detail.clone())
        .collect::<Vec<_>>()
        .join("; ")
}

fn numbers(text: &str) -> BTreeSet<String> {
    DIGITS
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

// ─── The check before a card ─────────────────────────────────────────────────

/// Actions whose payload "text" Nary wrote to a customer (checked before the card is made).
pub const CHECKED_ACTIONS: &[&str] = &[
    "whatsapp.reply.send",
    "email.reply.send",
    "reengagement.send",
    "price.answer.send",
    "cadence.followup.send",
    "lost.reply.send",
    "summary.email.send",
    "whatsapp.ack.send",
    "email.ack.send",
    "meeting.slots.send",
    "meeting.reminder.send",
];

/// Nary's own messages that start the conversation again: they never offer a call or meeting (the customer
/// hasn't asked for one; Owner rule, 26 Oct 2026).
const PROACTIVE_ACTIONS: &[&str] = &[
    "cadence.followup.send",
    "reengagement.send",
    "lost.reply.send",
];
/// Setting: the patterns (regular expressions, any case) of a call or meeting offer.
pub const MEETING_PATTERNS: &str = "nary.writing.meeting_offer_patterns";

/// An offer of a call or meeting in `text`, by the patterns of `nary.writing.meeting_offer_patterns`.
pub async fn meeting_offer(db: &mut PgConnection, text: &str) -> ApiResult<Option<Finding>> {
    let raw = granary_core::settings_store::require_json(db, MEETING_PATTERNS, WHERE).await?;
    let Some(list) = raw.as_array() else {
        return Err(SettingError::new(MEETING_PATTERNS, "is not a list of patterns", WHERE).into());
    };
    for p in list.iter().filter_map(Value::as_str) {
        let re = Regex::new(&format!("(?i){p}")).map_err(|e| {
            SettingError::new(MEETING_PATTERNS, &format!("has a bad pattern ({e})"), WHERE)
        })?;
        if let Some(m) = re.find(text) {
            return Ok(Some(Finding {
                code: "meeting_offer",
                detail: format!(
                    "offers a call or meeting the customer didn't ask for (\"{}\")",
                    m.as_str()
                ),
            }));
        }
    }
    Ok(None)
}

/// Template texts (from settings): flagged, never rewritten, never held for it.
const TEMPLATE_ACTIONS: &[&str] = &[
    "whatsapp.ack.send",
    "email.ack.send",
    "meeting.slots.send",
    "meeting.reminder.send",
];
/// Messages in the middle of a conversation: no time greeting expected.
const NO_GREETING_ACTIONS: &[&str] = &["meeting.slots.send", "meeting.reminder.send"];

/// What the check decided for a card.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Review {
    /// A line for the card (what was fixed, or what is still wrong).
    pub note: Option<String>,
    /// The problems left: the card must wait for the Owner.
    pub hold: bool,
    pub findings: Vec<String>,
}

fn channel_of(action: &str, payload: &Value) -> &'static str {
    match payload.get("channel").and_then(Value::as_str) {
        Some("email") => return "email",
        Some("whatsapp") => return "whatsapp",
        _ => {}
    }
    if action.starts_with("email.") || action == "summary.email.send" {
        "email"
    } else if payload
        .get("chat_id")
        .and_then(Value::as_str)
        .is_some_and(|c| !c.is_empty())
        || action.starts_with("whatsapp.")
        || action == "reengagement.send"
    {
        "whatsapp"
    } else {
        "email"
    }
}

/// `nary.writing.lint_enabled`: "true"/"false"; missing is a clear setting error.
pub async fn enabled(db: &mut PgConnection) -> ApiResult<bool> {
    match get_optional(db, LINT_SETTING).await? {
        Some(v) => Ok(pystr::lower(&v) == "true"),
        None => Err(SettingError::new(LINT_SETTING, "is not set", WHERE).into()),
    }
}

/// The rewrite schema.
pub fn fix_schema() -> Value {
    json!({"type": "object", "properties": {"text": {"type": "string"}},
           "required": ["text"], "additionalProperties": false})
}

/// One rewrite of `text` fixing `findings`, or None (no answer, empty).
pub async fn rewrite(
    db: &mut PgConnection,
    channel: &str,
    language: &str,
    text: &str,
    findings: &[Finding],
) -> ApiResult<Option<String>> {
    let rules = format!(
        "Rewrite the customer message below so it follows the writing rules. Fix only these problems: {}. \
         Keep the meaning, the language ({}), every fact, name, price, number and date exactly; keep it as \
         short as it is. Return the whole message in 'text'.",
        describe(findings),
        if language == "en" || language.is_empty() { "English" } else { language }
    );
    let system = customer_prompt(db, Some(channel), &rules, "").await?;
    let content = super::llm::information_default("draft", text);
    let data = super::llm::ask_json(
        db,
        FIX_JOB,
        &system,
        &content,
        &fix_schema(),
        None,
        None,
        None,
    )
    .await?;
    Ok(data
        .as_ref()
        .and_then(|d| d.get("text"))
        .and_then(Value::as_str)
        .map(|t| pystr::strip(t).to_string())
        .filter(|t| !t.is_empty()))
}

/// The check before a customer card: lint the payload's text; on a hit rewrite it once (the new text goes in
/// the payload when it passes and keeps every number); otherwise the card carries the problems and waits for
/// the Owner. Templates (acknowledgements) are only flagged. A text the Owner wrote (`by_owner`) is never touched.
pub async fn review_card(
    db: &mut PgConnection,
    action: &str,
    payload: &mut Value,
) -> ApiResult<Review> {
    if !CHECKED_ACTIONS.contains(&action) {
        return Ok(Review::default());
    }
    if payload
        .get("by_owner")
        .is_some_and(crate::whatsapp::py_truthy)
    {
        return Ok(Review::default());
    }
    let Some(text) = payload
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|t| !pystr::strip(t).is_empty())
    else {
        return Ok(Review::default());
    };
    if !enabled(db).await? {
        return Ok(Review::default());
    }
    let channel = channel_of(action, payload);
    let language = payload
        .get("language")
        .and_then(Value::as_str)
        .filter(|l| *l == "en")
        .map(str::to_string)
        .unwrap_or_else(|| guess_language(&text).to_string());
    // an attachment on the card, or the commitments of a call record (the summary email)
    let document_confirmed = action == "summary.email.send"
        || payload
            .get("attach")
            .is_some_and(crate::whatsapp::py_truthy);
    fn input<'a>(
        t: &'a str,
        channel: &'a str,
        language: &'a str,
        confirmed: bool,
    ) -> LintInput<'a> {
        LintInput {
            text: t,
            channel,
            language: Some(language),
            document_confirmed: confirmed,
        }
    }
    let mut found = lint(input(&text, channel, &language, document_confirmed));
    if NO_GREETING_ACTIONS.contains(&action) {
        found.retain(|f| f.code != "greeting");
    }
    let proactive = PROACTIVE_ACTIONS.contains(&action);
    if proactive {
        found.extend(meeting_offer(db, &text).await?);
    }
    if found.is_empty() {
        return Ok(Review::default());
    }
    if TEMPLATE_ACTIONS.contains(&action) {
        tracing::warn!(action, problems = %describe(&found), "nary_writing_template_flagged");
        return Ok(Review {
            note: Some(format!("Writing check: {}", describe(&found))),
            hold: false,
            findings: found.iter().map(|f| f.code.to_string()).collect(),
        });
    }
    let fixed = match Box::pin(rewrite(db, channel, &language, &text, &found)).await {
        Ok(t) => t,
        // the rewrite is a second chance, never a reason to lose the card
        Err(e) => {
            tracing::warn!(action, error = %e, "nary_writing_rewrite_failed");
            None
        }
    };
    if let Some(new_text) = fixed {
        let mut left = lint(input(&new_text, channel, &language, document_confirmed));
        if proactive {
            left.extend(meeting_offer(db, &new_text).await?);
        }
        if left.is_empty() && numbers(&new_text) == numbers(&text) {
            payload["text"] = Value::String(new_text);
            return Ok(Review {
                note: Some(format!(
                    "Writing check: rewritten once ({})",
                    describe(&found)
                )),
                hold: false,
                findings: vec![],
            });
        }
    }
    Ok(Review {
        note: Some(format!(
            "Writing check, please look before sending: {}",
            describe(&found)
        )),
        hold: true,
        findings: found.iter().map(|f| f.code.to_string()).collect(),
    })
}
