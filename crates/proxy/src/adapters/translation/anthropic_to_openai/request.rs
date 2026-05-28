//! Translate Anthropic /v1/messages request body to OpenAI /v1/chat/completions.

use serde_json::{Value, json};

use crate::application::errors::ProxyError;

pub fn translate(body: &[u8]) -> Result<Vec<u8>, ProxyError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| ProxyError::TranslationInvalidRequest {
            field: "body",
            reason: format!("parse failed: {e}"),
        })?;
    let translated = translate_value(&v)?;
    serde_json::to_vec(&translated).map_err(|e| ProxyError::TranslationInvalidRequest {
        field: "body",
        reason: format!("serialize failed: {e}"),
    })
}

fn translate_value(v: &Value) -> Result<Value, ProxyError> {
    let mut out = json!({});
    let m = out.as_object_mut().unwrap();

    // model — passthrough
    if let Some(model) = v.get("model") {
        m.insert("model".into(), model.clone());
    }

    // max_tokens — passthrough (required by Anthropic, optional by OpenAI)
    if let Some(mt) = v.get("max_tokens") {
        m.insert("max_tokens".into(), mt.clone());
    }

    // sampling — passthrough except top_k (drop) and stop_sequences (rename to stop)
    for k in ["temperature", "top_p", "stream"] {
        if let Some(val) = v.get(k) {
            m.insert(k.into(), val.clone());
        }
    }
    if let Some(stops) = v.get("stop_sequences") {
        m.insert("stop".into(), stops.clone());
    }
    // top_k: drop with no error.

    // metadata.user_id → user
    if let Some(user) = v.pointer("/metadata/user_id") {
        m.insert("user".into(), user.clone());
    }

    // tools — wrap each in function envelope
    if let Some(tools) = v.get("tools").and_then(|t| t.as_array()) {
        let translated_tools: Vec<Value> = tools.iter().map(translate_tool_def).collect();
        m.insert("tools".into(), Value::Array(translated_tools));
    }

    // tool_choice
    if let Some(tc) = v.get("tool_choice") {
        m.insert("tool_choice".into(), translate_tool_choice(tc));
    }

    // messages: build OpenAI messages from Anthropic system + messages
    let messages = build_openai_messages(v)?;
    m.insert("messages".into(), Value::Array(messages));

    Ok(out)
}

fn translate_tool_def(tool: &Value) -> Value {
    let name = tool
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("<unnamed>");
    let raw_params = tool.get("input_schema").cloned().unwrap_or(json!({}));
    let parameters = strip_schema_keywords(raw_params.clone());

    if parameters != raw_params {
        tracing::debug!(
            target: "translation",
            tool = name,
            "tool schema was modified by strip_schema_keywords"
        );
    }

    json!({
        "type": "function",
        "function": {
            "name": tool.get("name").cloned().unwrap_or(Value::Null),
            "description": tool.get("description").cloned().unwrap_or(Value::Null),
            "parameters": parameters,
        }
    })
}

/// Recursively sanitise a JSON Schema for OpenAI's function-calling API.
///
/// Two transforms:
/// 1. **Strip unsupported keywords** — OpenAI rejects keywords like `$schema`,
///    `propertyNames`, `title`, `default`, etc. with a 400.
/// 2. **Synthesise `required`** — OpenAI strict mode requires every key in
///    `properties` to appear in `required`.  Anthropic tools often omit
///    optional params (e.g. `button`, `notes`).  We fill `required` at every
///    level — top-level, nested `properties`, `additionalProperties`, `items`,
///    `anyOf` entries — so no level is missed.
///
/// We use a blocklist (not an allowlist) so that user-defined property names
/// inside `properties` objects are preserved.
fn strip_schema_keywords(mut v: Value) -> Value {
    if let Some(obj) = v.as_object_mut() {
        // JSON Schema keywords that OpenAI does NOT support.
        const UNSUPPORTED: &[&str] = &[
            // Meta-keywords
            "$schema", "$id", "$comment", "$defs", "definitions",
            // Metadata
            "title", "examples", "default", "deprecated",
            // Object constraints
            "propertyNames", "patternProperties", "minProperties", "maxProperties",
            // Array constraints
            "minItems", "maxItems", "uniqueItems", "contains", "minContains", "maxContains",
            // String constraints
            "minLength", "maxLength", "pattern", "format",
            // Number constraints
            "minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf",
            // Composition (OpenAI only supports `anyOf`)
            "allOf", "oneOf",
            // Conditional
            "if", "then", "else", "not",
            // Referencing
            "$ref", "$dynamicRef", "$recursiveRef",
            // Content
            "contentEncoding", "contentMediaType",
            // Other
            "readOnly", "writeOnly",
        ];
        for keyword in UNSUPPORTED {
            obj.remove(*keyword);
        }
        // Recurse into remaining values FIRST (depth-first) so that nested
        // objects are already clean before we fix `required` below.
        for (_, child) in obj.iter_mut() {
            *child = strip_schema_keywords(child.clone());
        }
        // Flatten `additionalProperties` that is an object schema (not a boolean).
        // Codex Responses API only accepts `additionalProperties: false` (boolean).
        // An object-valued `additionalProperties` like `{"type":"string"}` or
        // `{"type":"object","properties":{...}}` causes a schema rejection.
        // We collapse it to `false`.
        if let Some(ap) = obj.get("additionalProperties") {
            if !ap.is_boolean() {
                obj.insert("additionalProperties".into(), json!(false));
            }
        }
        // OpenAI strict mode requires every `type: "object"` with
        // `additionalProperties: false` to also have `properties` and `required`.
        // If an object property ends up with `additionalProperties: false` but
        // no `properties` (e.g. `answers` had only `additionalProperties:
        // {"type":"string"}`), inject empty ones so strict validation passes.
        if obj.get("type").and_then(|v| v.as_str()) == Some("object")
            && obj.get("additionalProperties").and_then(|v| v.as_bool()) == Some(false)
            && !obj.contains_key("properties")
        {
            obj.insert("properties".into(), json!({}));
            obj.insert("required".into(), json!([]));
        }
        // After recursion: ensure `required` exactly matches `properties` keys.
        // OpenAI strict mode requires a bidirectional match: every key in
        // properties must be in required AND every key in required must be in
        // properties.
        if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
            let all_keys: Vec<Value> = props.keys().map(|k| Value::String(k.clone())).collect();
            obj.insert("required".into(), Value::Array(all_keys));
        } else {
            // No properties — remove required to avoid referencing phantom keys.
            obj.remove("required");
        }
    }
    if let Some(arr) = v.as_array_mut() {
        for child in arr.iter_mut() {
            *child = strip_schema_keywords(child.clone());
        }
    }
    v
}

fn translate_tool_choice(tc: &Value) -> Value {
    match tc {
        Value::String(s) if s == "auto" => json!("auto"),
        Value::String(s) if s == "any" => json!("required"),
        Value::String(s) if s == "none" => json!("none"),
        Value::Object(map) if map.get("type").and_then(|t| t.as_str()) == Some("tool") => {
            json!({"type": "function", "function": {"name": map.get("name").cloned().unwrap_or(Value::Null)}})
        }
        other => other.clone(),
    }
}

fn build_openai_messages(req: &Value) -> Result<Vec<Value>, ProxyError> {
    let mut out = Vec::new();

    // System: top-level field becomes a message.
    if let Some(sys) = req.get("system") {
        let sys_text = flatten_text(sys);
        if !sys_text.is_empty() {
            out.push(json!({"role": "system", "content": sys_text}));
        }
    }

    if let Some(msgs) = req.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let translated = translate_message(msg)?;
            for m in translated {
                out.push(m);
            }
        }
    }

    Ok(out)
}

/// One Anthropic message may produce multiple OpenAI messages
/// (e.g. tool_result blocks become separate role=tool messages).
fn translate_message(msg: &Value) -> Result<Vec<Value>, ProxyError> {
    let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
    let content = msg.get("content").cloned().unwrap_or(Value::Null);

    match &content {
        Value::String(s) => Ok(vec![json!({"role": role, "content": s})]),
        Value::Array(blocks) => {
            let mut text_parts = Vec::<String>::new();
            let mut tool_calls = Vec::<Value>::new();
            let mut tool_messages = Vec::<Value>::new();

            for block in blocks {
                let btype = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match btype {
                    "text" => {
                        if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                            text_parts.push(t.to_string());
                        }
                    }
                    "tool_use" => {
                        let id = block.get("id").cloned().unwrap_or(Value::String("".into()));
                        let name = block.get("name").cloned().unwrap_or(Value::Null);
                        let input = block.get("input").cloned().unwrap_or(json!({}));
                        let args = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
                        tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": args}
                        }));
                    }
                    "tool_result" => {
                        let tool_use_id = block.get("tool_use_id").cloned().unwrap_or(Value::Null);
                        let tr_content = block.get("content").cloned().unwrap_or(Value::Null);
                        let flat = flatten_text(&tr_content);
                        tool_messages.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": flat,
                        }));
                    }
                    "image" => {
                        // out of scope: drop with warning
                        tracing::warn!(target: "translation", "image block dropped (out of scope)");
                    }
                    other => {
                        tracing::debug!(target: "translation", block_type = %other, "unknown block type, dropped");
                    }
                }
            }

            let mut result = Vec::new();
            if !text_parts.is_empty() || !tool_calls.is_empty() {
                let mut msg_out = json!({"role": role});
                let mo = msg_out.as_object_mut().unwrap();
                if !text_parts.is_empty() {
                    mo.insert("content".into(), Value::String(text_parts.join("\n")));
                } else {
                    mo.insert("content".into(), Value::Null);
                }
                if !tool_calls.is_empty() {
                    mo.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                result.push(msg_out);
            }
            result.extend(tool_messages);
            Ok(result)
        }
        _ => Ok(vec![]), // unrecognized content type
    }
}

/// Flatten a JSON value to plain text. String → itself; array of {type:"text", text} → joined.
fn flatten_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                    parts.push(t.to_string());
                } else if let Some(s) = item.as_str() {
                    parts.push(s.to_string());
                }
            }
            parts.join("\n")
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_message() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["model"], "x");
        assert_eq!(out["max_tokens"], 10);
        assert_eq!(out["messages"][0]["role"], "user");
        assert_eq!(out["messages"][0]["content"], "hi");
    }

    #[test]
    fn system_string_becomes_first_message() {
        let body = br#"{"model":"x","max_tokens":10,"system":"You are X","messages":[{"role":"user","content":"hi"}]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["messages"][0]["role"], "system");
        assert_eq!(out["messages"][0]["content"], "You are X");
        assert_eq!(out["messages"][1]["role"], "user");
    }

    #[test]
    fn system_array_blocks_join() {
        let body = br#"{"model":"x","max_tokens":10,"system":[{"type":"text","text":"a"},{"type":"text","text":"b"}],"messages":[]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["messages"][0]["content"], "a\nb");
    }

    #[test]
    fn tool_use_becomes_tool_calls() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[
            {"role":"assistant","content":[
                {"type":"text","text":"calling tool"},
                {"type":"tool_use","id":"t1","name":"search","input":{"q":"hello"}}
            ]}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["messages"][0]["role"], "assistant");
        assert_eq!(out["messages"][0]["content"], "calling tool");
        assert_eq!(out["messages"][0]["tool_calls"][0]["id"], "t1");
        assert_eq!(out["messages"][0]["tool_calls"][0]["type"], "function");
        assert_eq!(
            out["messages"][0]["tool_calls"][0]["function"]["name"],
            "search"
        );
        assert!(
            out["messages"][0]["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap()
                .contains("hello")
        );
    }

    #[test]
    fn tool_result_becomes_role_tool_message() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"t1","content":"42"}
            ]}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["messages"][0]["role"], "tool");
        assert_eq!(out["messages"][0]["tool_call_id"], "t1");
        assert_eq!(out["messages"][0]["content"], "42");
    }

    #[test]
    fn tools_wrap_in_function_envelope() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"tools":[
            {"name":"search","description":"web search","input_schema":{"type":"object"}}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["tools"][0]["type"], "function");
        assert_eq!(out["tools"][0]["function"]["name"], "search");
        assert_eq!(out["tools"][0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn strip_schema_keywords_removes_dollar_schema() {
        let body = r#"{"model":"x","max_tokens":10,"messages":[],"tools":[
            {"name":"Read","description":"Read a file","input_schema":{
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "filePath": {"$schema": "nested", "type": "string"}
                },
                "required": ["filePath"],
                "additionalProperties": false
            }}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body.as_bytes()).unwrap()).unwrap();
        let params = &out["tools"][0]["function"]["parameters"];
        // $schema must be removed at root level
        assert!(
            params.get("$schema").is_none(),
            "$schema should be stripped from parameters"
        );
        // $schema must be removed from nested properties
        let file_path_prop = &params["properties"]["filePath"];
        assert!(
            file_path_prop.get("$schema").is_none(),
            "$schema should be stripped from nested properties"
        );
        // But actual schema fields must remain intact
        assert_eq!(params["type"], "object");
        assert_eq!(params["required"], json!(["filePath"]));
        assert_eq!(file_path_prop["type"], "string");
    }

    #[test]
    fn strip_schema_keywords_removes_dollar_id_and_comment() {
        let body = r#"{"model":"x","max_tokens":10,"messages":[],"tools":[
            {"name":"test","description":"test","input_schema":{
                "$id": "urn:test",
                "$comment": "a comment",
                "type": "object",
                "properties": {}
            }}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body.as_bytes()).unwrap()).unwrap();
        let params = &out["tools"][0]["function"]["parameters"];
        assert!(params.get("$id").is_none(), "$id should be stripped");
        assert!(params.get("$comment").is_none(), "$comment should be stripped");
        assert_eq!(params["type"], "object");
    }

    #[test]
    fn tool_choice_any_becomes_required() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"tool_choice":"any"}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["tool_choice"], "required");
    }

    #[test]
    fn tool_choice_named_rewraps() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"tool_choice":{"type":"tool","name":"search"}}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["tool_choice"]["type"], "function");
        assert_eq!(out["tool_choice"]["function"]["name"], "search");
    }

    #[test]
    fn stop_sequences_rename_to_stop() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"stop_sequences":["END"]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["stop"][0], "END");
        assert!(out.get("stop_sequences").is_none());
    }

    #[test]
    fn top_k_dropped() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"top_k":5}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert!(out.get("top_k").is_none());
    }

    #[test]
    fn cache_control_markers_dropped_from_text_blocks() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[
            {"role":"user","content":[{"type":"text","text":"hi","cache_control":{"type":"ephemeral"}}]}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        // cache_control is not in the output — text block contents are flattened to a plain string
        assert_eq!(out["messages"][0]["content"], "hi");
    }

    #[test]
    fn metadata_user_id_becomes_user() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"metadata":{"user_id":"u42"}}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["user"], "u42");
        assert!(out.get("metadata").is_none());
    }

    #[test]
    fn required_synthesised_from_properties_for_openai_strict_mode() {
        // Mimics Playwright's playwright_browser_click: `button` is in
        // properties but intentionally omitted from required.
        let body = r#"{"model":"x","max_tokens":10,"messages":[],"tools":[
            {"name":"click","description":"click","input_schema":{
                "type": "object",
                "properties": {
                    "target": {"type": "string"},
                    "button": {"type": "string", "default": "left"}
                },
                "required": ["target"]
            }}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body.as_bytes()).unwrap()).unwrap();
        let required = out["tools"][0]["function"]["parameters"]["required"]
            .as_array()
            .unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        // Must contain BOTH target and button — OpenAI strict mode requires it
        assert!(
            required_strs.contains(&"target"),
            "required should contain 'target'"
        );
        assert!(
            required_strs.contains(&"button"),
            "required should contain 'button' (was missing in Anthropic schema)"
        );
    }

    #[test]
    fn required_synthesised_when_missing_entirely() {
        // Tool with properties but no required array at all
        let body = r#"{"model":"x","max_tokens":10,"messages":[],"tools":[
            {"name":"foo","description":"bar","input_schema":{
                "type": "object",
                "properties": {
                    "a": {"type": "string"},
                    "b": {"type": "integer"}
                }
            }}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body.as_bytes()).unwrap()).unwrap();
        let required = out["tools"][0]["function"]["parameters"]["required"]
            .as_array()
            .unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_strs.contains(&"a"));
        assert!(required_strs.contains(&"b"));
    }

    #[test]
    fn strips_unsupported_keywords_like_property_names() {
        // Mimics Claude Code's AskUserQuestion tool which has `propertyNames`
        // — rejected by OpenAI with "is not permitted".
        let body = r#"{"model":"x","max_tokens":10,"messages":[],"tools":[
            {"name":"AskUserQuestion","description":"ask","input_schema":{
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {"type": "string"},
                                "options": {"type": "array", "items": {"type": "string"}}
                            }
                        }
                    }
                },
                "propertyNames": {"type": "string"},
                "title": "Questions",
                "default": {},
                "minProperties": 1,
                "examples": [{"questions": []}]
            }}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body.as_bytes()).unwrap()).unwrap();
        let params = &out["tools"][0]["function"]["parameters"];
        // Unsupported keywords must be stripped
        assert!(params.get("propertyNames").is_none(), "propertyNames must be stripped");
        assert!(params.get("title").is_none(), "title must be stripped");
        assert!(params.get("default").is_none(), "default must be stripped");
        assert!(params.get("minProperties").is_none(), "minProperties must be stripped");
        assert!(params.get("examples").is_none(), "examples must be stripped");
        // Supported keywords must remain
        assert_eq!(params["type"], "object");
        assert!(params.get("properties").is_some());
        assert!(params["properties"]["questions"]["items"].get("properties").is_some());
    }

    #[test]
    fn additionalProperties_object_schema_collapsed_to_false() {
        // Codex Responses API only accepts `additionalProperties: false` (boolean).
        // Object-valued `additionalProperties` causes schema rejection.
        let body = r#"{"model":"x","max_tokens":10,"messages":[],"tools":[
            {"name":"AskUserQuestion","description":"ask","input_schema":{
                "type": "object",
                "properties": {
                    "annotations": {
                        "type": "object",
                        "additionalProperties": {
                            "type": "object",
                            "properties": {
                                "value": {"type": "string"},
                                "notes": {"type": "string"}
                            },
                            "required": ["value"]
                        }
                    }
                }
            }}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body.as_bytes()).unwrap()).unwrap();
        let ap = &out["tools"][0]["function"]["parameters"]
            ["properties"]["annotations"]["additionalProperties"];
        // Must be collapsed to false, not an object schema
        assert_eq!(ap, &json!(false), "additionalProperties object schema must be collapsed to false");
    }

    #[test]
    fn phantom_required_keys_removed_when_not_in_properties() {
        // Anthropic schema has `annotations` in `required` but it's defined via
        // `patternProperties` (which we strip), not `properties`.  OpenAI:
        // "Extra required key 'annotations' supplied."
        let body = r#"{"model":"x","max_tokens":10,"messages":[],"tools":[
            {"name":"AskUserQuestion","description":"ask","input_schema":{
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "items": {"type": "string"}
                    }
                },
                "patternProperties": {
                    "annotations": {"type": "object"}
                },
                "required": ["questions", "annotations"]
            }}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body.as_bytes()).unwrap()).unwrap();
        let params = &out["tools"][0]["function"]["parameters"];
        let required = params["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            required_strs.contains(&"questions"),
            "required should contain 'questions'"
        );
        assert!(
            !required_strs.contains(&"annotations"),
            "annotations must NOT be in required (phantom key from patternProperties)"
        );
        // patternProperties itself must also be stripped
        assert!(
            params.get("patternProperties").is_none(),
            "patternProperties must be stripped"
        );
    }
}
