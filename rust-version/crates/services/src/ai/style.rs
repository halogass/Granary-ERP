//! Port of backend/app/services/ai/style.py: house style for everything the AI writes, no em dashes.
//!
//! Applied once, where every AI feature gets its model (`registry.get_provider`), so the assistant,
//! reply suggestions, drafts, briefs, AI fields, scoring, research summaries, document analysis and
//! connected agents all follow it. Transcribing an existing document (PDF to Markdown) keeps the
//! author's punctuation, so that path asks for a verbatim provider.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use fancy_regex::Regex;
use granary_compat::text::pystr::{self, PY_SPACE_CLASS};
use once_cell::sync::Lazy;
use serde_json::Value;

use crate::ai::base::{
    current_text_sink, with_text_sink, AiError, BatchOutcome, ChatRequest, ChatResult, FileRequest,
    LLMProvider, ModelList, TextSink, ToolCall,
};

pub const SETTING: &str = "ai.no_em_dash";

/// `\s*—\s*`
static EM_DASH: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!("[{PY_SPACE_CLASS}]*—[{PY_SPACE_CLASS}]*")).unwrap());
/// `([.!?:;,])\s*—\s*`
static AFTER_PUNCT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(&format!(
        "([.!?:;,])[{PY_SPACE_CLASS}]*—[{PY_SPACE_CLASS}]*"
    ))
    .unwrap()
});
/// `(?m)^(\s*)—\s*`
static LINE_START: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!("(?m)^([{PY_SPACE_CLASS}]*)—[{PY_SPACE_CLASS}]*")).unwrap());
/// `(```.*?```)` with re.S
static FENCE: Lazy<Regex> = Lazy::new(|| Regex::new("(?s)```.*?```").unwrap());

/// `enabled(cfg)`: the setting is on unless it says something other than "true".
pub fn enabled(cfg: &HashMap<String, String>) -> bool {
    let v = cfg.get(SETTING).map(String::as_str).unwrap_or("");
    let v = if v.is_empty() { "true" } else { v };
    pystr::lower(v) == "true"
}

/// `re.sub` with a replacement given as a closure over the captures (Python semantics for
/// non-overlapping, left-to-right matches; an empty match never happens with these patterns).
fn sub(re: &Regex, s: &str, rep: impl Fn(&fancy_regex::Captures) -> String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for caps in re.captures_iter(s) {
        let caps = caps.expect("regex backtrack limit");
        let m = caps.get(0).unwrap();
        out.push_str(&s[last..m.start()]);
        out.push_str(&rep(&caps));
        last = m.end();
    }
    out.push_str(&s[last..]);
    out
}

/// `strip_em_dashes(text)`: "a — b" to "a, b"; "done. — next" to "done. next"; a line starting with
/// one becomes a "- " bullet. Fenced code blocks are left as they are.
pub fn strip_em_dashes(text: &str) -> String {
    if text.is_empty() || !text.contains('—') {
        return text.to_string();
    }
    // _FENCE.split(text) with a capturing group: even parts are prose, odd parts are the fences.
    let mut parts: Vec<(bool, &str)> = Vec::new();
    let mut last = 0;
    for m in FENCE.find_iter(text) {
        let m = m.expect("regex backtrack limit");
        parts.push((false, &text[last..m.start()]));
        parts.push((true, m.as_str()));
        last = m.end();
    }
    parts.push((false, &text[last..]));
    let mut out = String::with_capacity(text.len());
    for (fence, part) in parts {
        if fence {
            out.push_str(part);
            continue;
        }
        let p = sub(&LINE_START, part, |c| format!("{}- ", &c[1]));
        let p = sub(&AFTER_PUNCT, &p, |c| format!("{} ", &c[1]));
        let p = sub(&EM_DASH, &p, |_| ", ".to_string());
        out.push_str(&p.replace(",,", ","));
    }
    out
}

/// `clean(value)`: strip em dashes from every string inside a value (tool-call arguments).
pub fn clean(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(strip_em_dashes(s)),
        Value::Array(a) => Value::Array(a.iter().map(clean).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), clean(v))).collect()),
        other => other.clone(),
    }
}

/// `StyledProvider`: wraps a provider so its answers follow the house style. Everything else passes
/// through (`__getattr__`).
pub struct StyledProvider {
    inner: Arc<dyn LLMProvider>,
}

impl StyledProvider {
    pub fn new(inner: Arc<dyn LLMProvider>) -> StyledProvider {
        StyledProvider { inner }
    }

    /// The wrapped provider.
    pub fn inner(&self) -> &dyn LLMProvider {
        self.inner.as_ref()
    }

    /// `_style(result)`: the text without em dashes, every tool call's arguments cleaned.
    pub fn style(result: ChatResult) -> ChatResult {
        let text = strip_em_dashes(&result.text);
        let tool_calls = result
            .tool_calls
            .into_iter()
            .map(|tc| ToolCall {
                args: clean(&tc.args),
                ..tc
            })
            .collect();
        ChatResult {
            text,
            tool_calls,
            ..result
        }
    }
}

#[async_trait]
impl LLMProvider for StyledProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn chat(&self, model: &str, req: &ChatRequest) -> Result<ChatResult, AiError> {
        // fix-stream: live text is styled on the way too, so what the chat shows is the styled answer.
        let Some(sink) = current_text_sink() else {
            return Ok(Self::style(self.inner.chat(model, req).await?));
        };
        let styler = std::sync::Arc::new(std::sync::Mutex::new(StreamStyler::new(sink)));
        let feed = styler.clone();
        let raw: TextSink = std::sync::Arc::new(move |d: &str| feed.lock().unwrap().feed(d));
        let result = Self::style(with_text_sink(raw, self.inner.chat(model, req)).await?);
        styler.lock().unwrap().finish(&result.text);
        Ok(result)
    }

    async fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, AiError> {
        self.inner.embed(model, texts).await
    }

    async fn analyze_file(&self, model: &str, req: &FileRequest) -> Result<ChatResult, AiError> {
        Ok(Self::style(self.inner.analyze_file(model, req).await?))
    }

    async fn list_models(&self) -> Result<ModelList, AiError> {
        self.inner.list_models().await
    }

    fn has_batch_api(&self) -> bool {
        self.inner.has_batch_api()
    }

    fn build_request(&self, model: &str, req: &ChatRequest) -> Result<Value, AiError> {
        self.inner.build_request(model, req)
    }

    async fn submit_batch(&self, requests: Vec<(String, Value)>) -> Result<String, AiError> {
        self.inner.submit_batch(requests).await
    }

    async fn batch_status(&self, batch_id: &str) -> Result<String, AiError> {
        self.inner.batch_status(batch_id).await
    }

    async fn collect_batch(&self, batch_id: &str) -> Result<Vec<(String, BatchOutcome)>, AiError> {
        self.inner.collect_batch(batch_id).await
    }

    /// `isinstance(self.provider, style.StyledProvider)` in `collect_batch`: the style is applied.
    fn style_result(&self, result: ChatResult) -> ChatResult {
        Self::style(result)
    }
}

// fix-stream: the house style on streamed text.

/// Styles text deltas as they arrive: it hands on `strip_em_dashes` of the longest prefix whose styled
/// form can no longer change: a prefix that ends on a character that is not a space, an em dash, a comma
/// or a backtick, outside any code fence, and before the first ",," (which a later em dash collapses).
/// What it hands on is checked to be a prefix of the styled whole, so the deltas add up to the styled
/// answer once `finish` has sent the rest. Should the two ever part, it stops and `finish` sends the
/// styled remainder of the raw text instead (no text twice, no em dash).
pub struct StreamStyler {
    out: TextSink,
    raw: String,
    emitted: String,
    /// The end of the raw prefix `emitted` is the styled form of.
    cut: usize,
    stopped: bool,
}

impl StreamStyler {
    pub fn new(out: TextSink) -> StreamStyler {
        StreamStyler {
            out,
            raw: String::new(),
            emitted: String::new(),
            cut: 0,
            stopped: false,
        }
    }

    /// One raw delta from the model.
    pub fn feed(&mut self, delta: &str) {
        self.raw.push_str(delta);
        if self.stopped {
            return;
        }
        let cut = safe_cut(&self.raw);
        if cut <= self.cut {
            return;
        }
        let styled = strip_em_dashes(&self.raw[..cut]);
        match styled.strip_prefix(self.emitted.as_str()) {
            Some(add) => {
                if !add.is_empty() {
                    (self.out)(add);
                    self.emitted.push_str(add);
                }
                self.cut = cut;
            }
            None => self.stopped = true,
        }
    }

    /// The call is over and `styled` is its styled text: sends what is still missing. Nothing streamed,
    /// nothing sent (the caller shows the answer itself).
    pub fn finish(&mut self, styled: &str) {
        if self.raw.is_empty() {
            return;
        }
        let rest = match styled.strip_prefix(self.emitted.as_str()) {
            Some(rest) => rest.to_string(),
            None => strip_em_dashes(&self.raw[self.cut..]),
        };
        if !rest.is_empty() {
            (self.out)(&rest);
            self.emitted.push_str(&rest);
        }
    }
}

/// The end of the longest prefix of `raw` whose styled form is final (see [`StreamStyler`]).
fn safe_cut(raw: &str) -> usize {
    let mut limit = raw.find(",,").unwrap_or(raw.len());
    loop {
        let fences: Vec<usize> = raw[..limit].match_indices("```").map(|(i, _)| i).collect();
        if fences.len() % 2 == 1 {
            limit = *fences.last().unwrap();
            continue;
        }
        let cut = raw[..limit]
            .char_indices()
            .rev()
            .find(|(_, c)| !(c.is_whitespace() || matches!(c, '\u{2014}' | ',' | '`')))
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        if raw[..cut].matches("```").count().is_multiple_of(2) {
            return cut;
        }
        limit = cut;
    }
}
