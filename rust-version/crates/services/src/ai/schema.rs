//! Structured-output schemas as the Anthropic API accepts them (Nary fix batch 1, C1).
//!
//! The API refuses some JSON Schema that other providers take. The case seen in production: a nullable
//! enum written as `{"type": ["string", "null"], "enum": ["custom", ..., null]}` is a 400
//! invalid_request_error ("Enum value 'custom' does not match declared type ['string','null']"), and the
//! step failed for every message. [`for_anthropic`] rewrites a schema into the accepted form just before it
//! is sent (a nullable enum becomes `anyOf [{type: string, enum: [...]}, {type: null}]`, an object with
//! properties gets `additionalProperties: false`, unsupported constraints move into the description), and
//! [`violations`] lists what the API would refuse, for the test that checks every schema Nary uses.

use serde_json::{json, Map, Value};

/// Types the API knows.
const TYPES: &[&str] = &[
    "object", "array", "string", "integer", "number", "boolean", "null",
];
/// Keywords the API refuses (numeric, string and array constraints, conditionals).
const UNSUPPORTED: &[&str] = &[
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "maxProperties",
    "patternProperties",
    "if",
    "then",
    "else",
    "not",
    "oneOf",
    "dependentRequired",
    "dependentSchemas",
];
const FORMATS: &[&str] = &[
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "uri",
    "ipv4",
    "ipv6",
    "uuid",
];

fn type_list(node: &Map<String, Value>) -> Option<Vec<String>> {
    match node.get("type") {
        Some(Value::String(t)) => Some(vec![t.clone()]),
        Some(Value::Array(a)) => Some(
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        ),
        _ => None,
    }
}

fn json_type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn matches_type(v: &Value, t: &str) -> bool {
    let vt = json_type_of(v);
    vt == t || (t == "number" && vt == "integer")
}

/// The schema rewritten into the form the Anthropic API accepts (see the module notes). A schema that is
/// already fine comes back unchanged.
pub fn for_anthropic(schema: &Value) -> Value {
    match schema {
        Value::Object(node) => Value::Object(fix_node(node)),
        Value::Array(a) => Value::Array(a.iter().map(for_anthropic).collect()),
        other => other.clone(),
    }
}

fn fix_children(node: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in node {
        let fixed = match k.as_str() {
            "properties" | "$defs" | "definitions" => match v {
                Value::Object(props) => Value::Object(
                    props
                        .iter()
                        .map(|(pk, pv)| (pk.clone(), for_anthropic(pv)))
                        .collect(),
                ),
                other => other.clone(),
            },
            "items" | "anyOf" | "allOf" | "prefixItems" => for_anthropic(v),
            _ => v.clone(),
        };
        out.insert(k.clone(), fixed);
    }
    out
}

fn fix_node(node: &Map<String, Value>) -> Map<String, Value> {
    let mut node = fix_children(node);
    // unsupported constraints: kept as words in the description
    let mut notes = Vec::new();
    for k in UNSUPPORTED {
        if let Some(v) = node.remove(*k) {
            notes.push(format!("{k}: {v}"));
        }
    }
    if let Some(Value::Number(n)) = node.get("minItems") {
        if n.as_u64().is_some_and(|m| m > 1) {
            let v = node.remove("minItems").unwrap_or(Value::Null);
            notes.push(format!("minItems: {v}"));
        }
    }
    if let Some(Value::String(f)) = node.get("format") {
        if !FORMATS.contains(&f.as_str()) {
            let v = node.remove("format").unwrap_or(Value::Null);
            notes.push(format!("format: {v}"));
        }
    }
    if !notes.is_empty() {
        let d = node
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let joined = notes.join(", ");
        node.insert(
            "description".into(),
            Value::String(if d.is_empty() {
                format!("{{{joined}}}")
            } else {
                format!("{d}\n\n{{{joined}}}")
            }),
        );
    }
    let types = type_list(&node);
    let is_object = types
        .as_ref()
        .is_some_and(|t| t.iter().any(|x| x == "object"));
    if is_object && node.contains_key("properties") && !node.contains_key("additionalProperties") {
        node.insert("additionalProperties".into(), Value::Bool(false));
    }
    // an enum with a list of types: one branch per type, each with its own enum values
    if let (Some(types), Some(Value::Array(values))) = (types.as_ref(), node.get("enum").cloned()) {
        if types.len() > 1 {
            node.remove("type");
            node.remove("enum");
            let description = node.remove("description");
            let mut branches = Vec::new();
            for t in types {
                if t == "null" {
                    branches.push(json!({"type": "null"}));
                    continue;
                }
                let vals: Vec<Value> = values
                    .iter()
                    .filter(|v| matches_type(v, t))
                    .cloned()
                    .collect();
                if vals.is_empty() {
                    continue;
                }
                let mut b = Map::new();
                b.insert("type".into(), Value::String(t.clone()));
                b.insert("enum".into(), Value::Array(vals));
                branches.push(Value::Object(b));
            }
            let mut out = Map::new();
            if let Some(d) = description {
                out.insert("description".into(), d);
            }
            out.insert("anyOf".into(), Value::Array(branches));
            for (k, v) in node {
                out.insert(k, v);
            }
            return out;
        }
    }
    node
}

/// What the Anthropic API would refuse in a structured-output schema, one line per problem with its path
/// (empty when the schema is accepted).
pub fn violations(schema: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk(schema, "$", &mut out);
    out
}

fn walk(v: &Value, path: &str, out: &mut Vec<String>) {
    let Value::Object(node) = v else {
        if !v.is_boolean() {
            out.push(format!("{path}: a schema must be an object"));
        }
        return;
    };
    let types = type_list(node);
    if let Some(t) = &types {
        for x in t {
            if !TYPES.contains(&x.as_str()) {
                out.push(format!("{path}: unknown type '{x}'"));
            }
        }
    }
    for k in UNSUPPORTED {
        if node.contains_key(*k) {
            out.push(format!("{path}: '{k}' is not supported"));
        }
    }
    if let Some(Value::Number(n)) = node.get("minItems") {
        if n.as_u64().is_none_or(|m| m > 1) {
            out.push(format!(
                "{path}: minItems other than 0 or 1 is not supported"
            ));
        }
    }
    if let Some(f) = node.get("format") {
        if !f.as_str().is_some_and(|f| FORMATS.contains(&f)) {
            out.push(format!("{path}: format {f} is not supported"));
        }
    }
    if let Some(e) = node.get("enum") {
        match e {
            Value::Array(values) if !values.is_empty() => {
                if let Some(t) = &types {
                    if t.len() > 1 {
                        out.push(format!(
                            "{path}: an enum with a list of types ({}) is refused; use anyOf",
                            t.join(", ")
                        ));
                    } else if let Some(t) = t.first() {
                        for val in values {
                            if !matches_type(val, t) {
                                out.push(format!(
                                    "{path}: enum value {val} does not match type '{t}'"
                                ));
                            }
                        }
                    }
                }
                for val in values {
                    if val.is_object() || val.is_array() {
                        out.push(format!("{path}: enum values must be simple ({val})"));
                    }
                }
            }
            _ => out.push(format!("{path}: enum must be a non-empty list")),
        }
    }
    let is_object = types
        .as_ref()
        .is_some_and(|t| t.iter().any(|x| x == "object"));
    // Claude refuses a free-form object (no properties) unless additionalProperties is false, which would
    // make it always empty: such data has to travel as JSON text instead (21 Oct, nary.proposal_edit).
    if is_object
        && node.get("properties").is_none()
        && node.get("additionalProperties") != Some(&Value::Bool(false))
    {
        out.push(format!(
            "{path}: a free-form object is refused; send it as a JSON string"
        ));
    }
    if let Some(props) = node.get("properties") {
        let Value::Object(props) = props else {
            out.push(format!("{path}: properties must be an object"));
            return;
        };
        if is_object && node.get("additionalProperties") != Some(&Value::Bool(false)) {
            out.push(format!(
                "{path}: an object must set additionalProperties to false"
            ));
        }
        for (k, p) in props {
            walk(p, &format!("{path}.{k}"), out);
        }
        if let Some(req) = node.get("required") {
            match req {
                Value::Array(r) => {
                    for k in r {
                        if !k.as_str().is_some_and(|k| props.contains_key(k)) {
                            out.push(format!("{path}: required {k} is not a property"));
                        }
                    }
                }
                _ => out.push(format!("{path}: required must be a list")),
            }
        }
    }
    if let Some(ap) = node.get("additionalProperties") {
        if ap != &Value::Bool(false) {
            out.push(format!(
                "{path}: additionalProperties other than false is not supported"
            ));
        }
    }
    if let Some(items) = node.get("items") {
        walk(items, &format!("{path}[]"), out);
    }
    for key in ["anyOf", "allOf"] {
        if let Some(list) = node.get(key) {
            match list {
                Value::Array(a) if !a.is_empty() => {
                    for (i, b) in a.iter().enumerate() {
                        walk(b, &format!("{path}.{key}[{i}]"), out);
                    }
                }
                _ => out.push(format!("{path}: {key} must be a non-empty list")),
            }
        }
    }
    if let Some(Value::String(r)) = node.get("$ref") {
        if !r.starts_with("#/") {
            out.push(format!("{path}: external $ref '{r}' is not supported"));
        }
    }
}

/// Is this provider error the API refusing the request itself (a 400 invalid_request_error)? Such a
/// call fails the same way every time, so it is never retried.
pub fn is_invalid_request(e: &crate::ai::base::AiError) -> bool {
    match e {
        crate::ai::base::AiError::Provider(p) => {
            p.status == Some(400)
                && p.body
                    .as_ref()
                    .and_then(|b| b.get("error").or(Some(b)))
                    .and_then(|e| e.get("type"))
                    .and_then(Value::as_str)
                    .is_none_or(|t| t == "invalid_request_error")
        }
        _ => false,
    }
}

/// Structured output for the OpenAI-compatible `response_format: {type: json_schema}` (batch F): the
/// Anthropic rewrite (nullable enums as anyOf, `additionalProperties: false`, unsupported constraints as
/// words) is also what OpenAI's strict mode accepts, so it is reused. Returns the schema and whether
/// `strict` can be asked for: strict mode needs every property of every object listed in `required` and
/// an object at the root, so a schema with optional fields is sent non-strict (still followed, not
/// enforced).
pub fn for_openai(schema: &Value) -> (Value, bool) {
    let fixed = for_anthropic(schema);
    let root_is_object = fixed
        .as_object()
        .and_then(type_list)
        .is_some_and(|t| t == ["object"]);
    let strict = root_is_object && all_required(&fixed);
    (fixed, strict)
}

fn all_required(v: &Value) -> bool {
    let Value::Object(node) = v else {
        return true;
    };
    if node.contains_key("$ref") || node.contains_key("allOf") {
        return false;
    }
    if let Some(Value::Object(props)) = node.get("properties") {
        let required: Vec<&str> = match node.get("required") {
            Some(Value::Array(r)) => r.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if props.keys().any(|k| !required.contains(&k.as_str())) {
            return false;
        }
        if !props.values().all(all_required) {
            return false;
        }
    }
    if let Some(items) = node.get("items") {
        if !all_required(items) {
            return false;
        }
    }
    if let Some(Value::Array(branches)) = node.get("anyOf") {
        if !branches.iter().all(all_required) {
            return false;
        }
    }
    true
}

/// Keywords Gemini's `responseSchema` (its OpenAPI 3.0 subset) takes as they are.
const GEMINI_KEEP: &[&str] = &[
    "description",
    "title",
    "minItems",
    "maxItems",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minProperties",
    "maxProperties",
    "pattern",
    "default",
    "example",
];
/// Keywords handled by the conversion itself (everything else is moved into the description).
const GEMINI_HANDLED: &[&str] = &[
    "type",
    "enum",
    "const",
    "nullable",
    "format",
    "properties",
    "required",
    "items",
    "anyOf",
    "oneOf",
    "additionalProperties",
    "$schema",
    "$id",
    "$defs",
    "definitions",
];

/// Structured output for Gemini's `generationConfig.responseSchema` (batch F). Gemini takes an OpenAPI
/// subset: upper-case types, `nullable` instead of a `null` type, string-only enums, `anyOf` but not
/// `oneOf`/`allOf`/`$ref`/`const`/`additionalProperties`. This converts a JSON Schema into that form:
/// a `null` type or branch becomes `nullable: true`, `oneOf` becomes `anyOf`, `const` a one-value enum,
/// local `$ref`s are inlined, and what has no Gemini form is kept as words in the description so the
/// model still sees it. Objects get `propertyOrdering` so the fields come back in the declared order.
pub fn for_gemini(schema: &Value) -> Value {
    let defs = schema
        .as_object()
        .and_then(|o| o.get("$defs").or_else(|| o.get("definitions")))
        .cloned()
        .unwrap_or(Value::Null);
    gemini_node(schema, &defs, 0)
}

fn gemini_type(t: &str) -> &'static str {
    match t {
        "string" => "STRING",
        "number" => "NUMBER",
        "integer" => "INTEGER",
        "boolean" => "BOOLEAN",
        "array" => "ARRAY",
        "object" => "OBJECT",
        _ => "STRING",
    }
}

fn resolve_ref<'a>(r: &str, defs: &'a Value) -> Option<&'a Value> {
    let name = r
        .strip_prefix("#/$defs/")
        .or_else(|| r.strip_prefix("#/definitions/"))?;
    defs.get(name)
}

fn gemini_node(v: &Value, defs: &Value, depth: usize) -> Value {
    let Value::Object(node) = v else {
        return json!({"type": "STRING"});
    };
    if depth > 32 {
        return json!({"type": "STRING"});
    }
    if let Some(Value::String(r)) = node.get("$ref") {
        if let Some(target) = resolve_ref(r, defs) {
            let mut merged = target.as_object().cloned().unwrap_or_default();
            for (k, val) in node {
                if k != "$ref" {
                    merged.insert(k.clone(), val.clone());
                }
            }
            return gemini_node(&Value::Object(merged), defs, depth + 1);
        }
    }
    let mut nullable = node.get("nullable") == Some(&Value::Bool(true));
    let mut notes: Vec<String> = Vec::new();

    // anyOf / oneOf: null branches become nullable; one branch left is merged in place.
    let branches = node
        .get("anyOf")
        .or_else(|| node.get("oneOf"))
        .and_then(Value::as_array);
    if let Some(branches) = branches {
        let mut rest: Vec<&Value> = Vec::new();
        for b in branches {
            if b.get("type").and_then(Value::as_str) == Some("null") {
                nullable = true;
            } else {
                rest.push(b);
            }
        }
        let mut out = if rest.len() == 1 {
            let mut merged = rest[0].as_object().cloned().unwrap_or_default();
            for (k, val) in node {
                if k != "anyOf" && k != "oneOf" && !merged.contains_key(k) {
                    merged.insert(k.clone(), val.clone());
                }
            }
            match gemini_node(&Value::Object(merged), defs, depth + 1) {
                Value::Object(o) => o,
                _ => Map::new(),
            }
        } else {
            let mut o = Map::new();
            if let Some(d) = node.get("description") {
                o.insert("description".into(), d.clone());
            }
            o.insert(
                "anyOf".into(),
                Value::Array(
                    rest.iter()
                        .map(|b| gemini_node(b, defs, depth + 1))
                        .collect(),
                ),
            );
            o
        };
        if nullable {
            out.insert("nullable".into(), Value::Bool(true));
        }
        return Value::Object(out);
    }

    // type: a list with null is nullable; several real types become anyOf.
    let mut types: Vec<String> = type_list(node).unwrap_or_default();
    if types.iter().any(|t| t == "null") {
        nullable = true;
        types.retain(|t| t != "null");
    }
    if types.len() > 1 {
        let mut o = Map::new();
        if let Some(d) = node.get("description") {
            o.insert("description".into(), d.clone());
        }
        let branches = types
            .iter()
            .map(|t| {
                let mut b = node.clone();
                b.insert("type".into(), json!(t));
                b.remove("description");
                if let Some(Value::Array(e)) = node.get("enum") {
                    b.insert(
                        "enum".into(),
                        Value::Array(e.iter().filter(|x| matches_type(x, t)).cloned().collect()),
                    );
                }
                gemini_node(&Value::Object(b), defs, depth + 1)
            })
            .collect();
        o.insert("anyOf".into(), Value::Array(branches));
        if nullable {
            o.insert("nullable".into(), Value::Bool(true));
        }
        return Value::Object(o);
    }
    let declared = types.first().cloned().or_else(|| {
        if node.contains_key("properties") {
            Some("object".into())
        } else if node.contains_key("items") {
            Some("array".into())
        } else {
            None
        }
    });
    let gtype = gemini_type(declared.as_deref().unwrap_or("string"));

    let mut out = Map::new();
    out.insert("type".into(), json!(gtype));
    for k in GEMINI_KEEP {
        if let Some(val) = node.get(*k) {
            out.insert((*k).into(), val.clone());
        }
    }
    // enum / const: Gemini enums are strings only.
    let mut values: Option<Vec<Value>> = match (node.get("enum"), node.get("const")) {
        (Some(Value::Array(e)), _) => Some(e.clone()),
        (None, Some(c)) => Some(vec![c.clone()]),
        _ => None,
    };
    if let Some(vals) = values.as_mut() {
        if vals.iter().any(Value::is_null) {
            nullable = true;
            vals.retain(|x| !x.is_null());
        }
    }
    if let Some(vals) = values.filter(|v| !v.is_empty()) {
        if gtype == "STRING" {
            out.insert(
                "enum".into(),
                Value::Array(
                    vals.iter()
                        .map(|x| match x {
                            Value::String(_) => x.clone(),
                            other => json!(other.to_string()),
                        })
                        .collect(),
                ),
            );
            out.insert("format".into(), json!("enum"));
        } else {
            let list: Vec<String> = vals.iter().map(Value::to_string).collect();
            notes.push(format!("one of: {}", list.join(", ")));
        }
    }
    if let Some(Value::String(f)) = node.get("format") {
        let ok = match gtype {
            "STRING" => matches!(f.as_str(), "date-time" | "enum"),
            "INTEGER" => matches!(f.as_str(), "int32" | "int64"),
            "NUMBER" => matches!(f.as_str(), "float" | "double"),
            _ => false,
        };
        if ok && !out.contains_key("format") {
            out.insert("format".into(), json!(f));
        } else if !ok {
            notes.push(format!("format: {f}"));
        }
    }
    if gtype == "OBJECT" {
        let props: Map<String, Value> = match node.get("properties") {
            Some(Value::Object(p)) => p
                .iter()
                .map(|(k, p)| (k.clone(), gemini_node(p, defs, depth + 1)))
                .collect(),
            _ => Map::new(),
        };
        if props.is_empty() {
            // Gemini refuses an OBJECT without properties: free-form data travels as JSON text.
            out.insert("type".into(), json!("STRING"));
            notes.push("a JSON object, as text".into());
        } else {
            let order: Vec<Value> = props.keys().map(|k| json!(k)).collect();
            if let Some(Value::Array(r)) = node.get("required") {
                let r: Vec<Value> = r
                    .iter()
                    .filter(|k| k.as_str().is_some_and(|k| props.contains_key(k)))
                    .cloned()
                    .collect();
                if !r.is_empty() {
                    out.insert("required".into(), Value::Array(r));
                }
            }
            out.insert("properties".into(), Value::Object(props));
            out.insert("propertyOrdering".into(), Value::Array(order));
        }
    }
    if gtype == "ARRAY" {
        let items = node
            .get("items")
            .cloned()
            .unwrap_or(json!({"type": "string"}));
        out.insert("items".into(), gemini_node(&items, defs, depth + 1));
    }
    // what Gemini has no field for: kept as words in the description
    for (k, val) in node {
        if !GEMINI_KEEP.contains(&k.as_str())
            && !GEMINI_HANDLED.contains(&k.as_str())
            && k != "$ref"
        {
            notes.push(format!("{k}: {val}"));
        }
    }
    if !notes.is_empty() {
        let d = out
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let joined = notes.join(", ");
        out.insert(
            "description".into(),
            json!(if d.is_empty() {
                format!("{{{joined}}}")
            } else {
                format!("{d}\n\n{{{joined}}}")
            }),
        );
    }
    if nullable {
        out.insert("nullable".into(), Value::Bool(true));
    }
    Value::Object(out)
}

/// The instruction added to the system prompt when the endpoint can only be asked for "a JSON object"
/// (`response_format: {type: json_object}`): the schema travels as text. OpenAI-compatible APIs also
/// refuse json_object mode unless the prompt mentions JSON, which this does.
pub fn json_instruction(schema: &Value) -> String {
    format!(
        "Reply with one JSON object only (no prose, no code fence) that matches this JSON Schema:\n{}",
        serde_json::to_string(schema).unwrap_or_default()
    )
}

/// What Gemini's `responseSchema` would refuse in a converted schema, one line per problem with its path
/// (empty when it is accepted): keywords outside its OpenAPI subset, lower-case or unknown types, an
/// OBJECT without properties, an ARRAY without items, a non-string enum.
pub fn gemini_violations(schema: &Value) -> Vec<String> {
    let mut out = Vec::new();
    gemini_walk(schema, "$", &mut out);
    out
}

const GEMINI_FIELDS: &[&str] = &[
    "type",
    "format",
    "description",
    "title",
    "nullable",
    "enum",
    "maxItems",
    "minItems",
    "properties",
    "required",
    "minProperties",
    "maxProperties",
    "minLength",
    "maxLength",
    "pattern",
    "example",
    "anyOf",
    "propertyOrdering",
    "default",
    "items",
    "minimum",
    "maximum",
];

fn gemini_walk(v: &Value, path: &str, out: &mut Vec<String>) {
    let Value::Object(node) = v else {
        out.push(format!("{path}: a schema must be an object"));
        return;
    };
    for k in node.keys() {
        if !GEMINI_FIELDS.contains(&k.as_str()) {
            out.push(format!("{path}: '{k}' is not a Gemini schema field"));
        }
    }
    let t = node.get("type").and_then(Value::as_str);
    match t {
        Some("STRING" | "NUMBER" | "INTEGER" | "BOOLEAN" | "ARRAY" | "OBJECT") => {}
        None if node.contains_key("anyOf") => {}
        other => out.push(format!("{path}: type {other:?} is not a Gemini type")),
    }
    if let Some(e) = node.get("enum") {
        if t != Some("STRING") {
            out.push(format!("{path}: enum on a non-STRING type"));
        }
        if !e
            .as_array()
            .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_string))
        {
            out.push(format!("{path}: enum must be a non-empty list of strings"));
        }
    }
    if t == Some("OBJECT") {
        match node.get("properties") {
            Some(Value::Object(p)) if !p.is_empty() => {
                for (k, p) in p {
                    gemini_walk(p, &format!("{path}.{k}"), out);
                }
            }
            _ => out.push(format!("{path}: an OBJECT needs properties")),
        }
    }
    if t == Some("ARRAY") {
        match node.get("items") {
            Some(items) => gemini_walk(items, &format!("{path}[]"), out),
            None => out.push(format!("{path}: an ARRAY needs items")),
        }
    }
    if let Some(Value::Array(a)) = node.get("anyOf") {
        for (i, b) in a.iter().enumerate() {
            gemini_walk(b, &format!("{path}.anyOf[{i}]"), out);
        }
    }
}
