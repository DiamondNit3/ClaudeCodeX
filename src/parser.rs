use crate::tools::ToolCall;
use anyhow::Result;
use serde_json::{json, Map, Value};

#[derive(Debug, Clone)]
pub struct ParsedToolCalls {
    pub calls: Vec<ToolCall>,
    pub visible_text: String,
}

pub fn parse_model_output(text: &str) -> Result<ParsedToolCalls> {
    let mut calls = Vec::new();
    let mut consumed = Vec::new();

    collect_xml_calls(text, &mut calls, &mut consumed)?;
    collect_fenced_json_calls(text, &mut calls, &mut consumed)?;
    collect_bare_json_calls(text, &mut calls)?;
    collect_malformed_action_calls(text, &mut calls);

    let visible_text = if calls.is_empty() {
        text.to_string()
    } else if consumed.is_empty() && text.contains("\"action\"") {
        String::new()
    } else {
        strip_ranges(text, &consumed).trim().to_string()
    };

    Ok(ParsedToolCalls {
        calls,
        visible_text,
    })
}

fn collect_xml_calls(
    text: &str,
    calls: &mut Vec<ToolCall>,
    consumed: &mut Vec<(usize, usize)>,
) -> Result<()> {
    let mut offset = 0;
    while let Some(relative_start) = text[offset..].find("<tool_call>") {
        let start = offset + relative_start;
        let content_start = start + "<tool_call>".len();
        let (content_end, block_end) = match text[content_start..].find("</tool_call>") {
            Some(relative_end) => {
                let content_end = content_start + relative_end;
                (content_end, content_end + "</tool_call>".len())
            }
            None => (text.len(), text.len()),
        };
        let raw = &text[content_start..content_end];
        if let Some(value) = first_json_value(raw) {
            if let Some(call) = value_to_tool_call(value)? {
                calls.push(call);
                consumed.push((start, block_end));
            }
        }
        offset = block_end;
    }
    Ok(())
}

fn collect_fenced_json_calls(
    text: &str,
    calls: &mut Vec<ToolCall>,
    consumed: &mut Vec<(usize, usize)>,
) -> Result<()> {
    let mut offset = 0;
    while let Some(relative_start) = text[offset..].find("```") {
        let start = offset + relative_start;
        let fence_body_start = match text[start + 3..].find('\n') {
            Some(newline) => start + 3 + newline + 1,
            None => break,
        };
        let Some(relative_end) = text[fence_body_start..].find("```") else {
            break;
        };
        let end = fence_body_start + relative_end;
        let raw = text[fence_body_start..end].trim();
        if let Ok(value) = serde_json::from_str::<Value>(raw) {
            if let Some(call) = value_to_tool_call(value)? {
                calls.push(call);
                consumed.push((start, end + 3));
            }
        }
        offset = end + 3;
    }
    Ok(())
}

fn collect_bare_json_calls(text: &str, calls: &mut Vec<ToolCall>) -> Result<()> {
    for raw in json_object_candidates(text) {
        if let Ok(value) = serde_json::from_str::<Value>(&raw) {
            if let Some(call) = value_to_tool_call(value)? {
                if !calls.iter().any(|existing| {
                    existing.tool == call.tool && existing.arguments == call.arguments
                }) {
                    calls.push(call);
                }
            }
        }
    }
    Ok(())
}

fn collect_malformed_action_calls(text: &str, calls: &mut Vec<ToolCall>) {
    if calls.iter().any(|call| call.tool == "write_file") {
        return;
    }
    if !(text.contains("\"action\":\"write_file\"") || text.contains("\"action\": \"write_file\""))
    {
        return;
    }

    let Some(path) = extract_json_string_field(text, "path") else {
        return;
    };
    let Some(content) = extract_content_field_lossy(text) else {
        return;
    };
    calls.push(ToolCall {
        tool: "write_file".to_string(),
        arguments: json!({
            "path": path,
            "content": content
        }),
        call_id: None,
    });
}

fn first_json_value(text: &str) -> Option<Value> {
    json_object_candidates(text)
        .into_iter()
        .find_map(|raw| serde_json::from_str::<Value>(&raw).ok())
}

fn json_object_candidates(text: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut start = None;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(start) = start.take() {
                            candidates.push(text[start..=index].to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    candidates
}

fn value_to_tool_call(value: Value) -> Result<Option<ToolCall>> {
    let Value::Object(mut object) = value else {
        return Ok(None);
    };

    if let Some(tool) = object
        .remove("tool")
        .and_then(|value| value.as_str().map(str::to_string))
    {
        let arguments = object.remove("arguments").unwrap_or_else(|| json!({}));
        let call_id = object
            .remove("call_id")
            .and_then(|value| value.as_str().map(str::to_string));
        return Ok(Some(ToolCall {
            tool,
            arguments,
            call_id,
        }));
    }

    if let Some(action) = object
        .remove("action")
        .and_then(|value| value.as_str().map(str::to_string))
    {
        let mut arguments = Map::new();
        for (key, value) in object {
            arguments.insert(key, value);
        }
        return Ok(Some(ToolCall {
            tool: action,
            arguments: Value::Object(arguments),
            call_id: None,
        }));
    }

    Ok(None)
}

fn extract_json_string_field(text: &str, field: &str) -> Option<String> {
    let start = string_field_value_start(text, field)?;
    let rest = &text[start..];
    let mut output = String::new();
    let mut escaped = false;
    for ch in rest.chars() {
        if escaped {
            output.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(output);
        } else {
            output.push(ch);
        }
    }
    None
}

fn extract_content_field_lossy(text: &str) -> Option<String> {
    let start = string_field_value_start(text, "content")?;
    let rest = &text[start..];
    let end = rest.rfind("\"}").or_else(|| rest.rfind("\"\n}"));
    let mut content = match end {
        Some(end) => rest[..end].trim().to_string(),
        None => rest
            .trim()
            .trim_end_matches('}')
            .trim_end_matches('"')
            .to_string(),
    };
    if let Some(duplicate_start) = content.find("\n{\"action\"") {
        content.truncate(duplicate_start);
    }
    Some(unescape_jsonish(&content))
}

fn string_field_value_start(text: &str, field: &str) -> Option<usize> {
    let marker = format!("\"{field}\"");
    let field_start = text.find(&marker)? + marker.len();
    let after_field = &text[field_start..];
    let colon = field_start + after_field.find(':')?;
    let after_colon = &text[colon + 1..];
    let quote = after_colon.find('"')?;
    Some(colon + 1 + quote + 1)
}

fn unescape_jsonish(text: &str) -> String {
    let mut output = String::new();
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            output.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            output.push(ch);
        }
    }
    output
}

fn strip_ranges(text: &str, ranges: &[(usize, usize)]) -> String {
    if ranges.is_empty() {
        return text.to_string();
    }

    let mut output = String::new();
    let mut cursor = 0;
    for (start, end) in ranges {
        if *start >= cursor {
            output.push_str(&text[cursor..*start]);
            cursor = *end;
        }
    }
    output.push_str(&text[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_xml_tool_call() {
        let parsed = parse_model_output(
            r#"<tool_call>{"tool":"read_file","arguments":{"path":"Cargo.toml"}}</tool_call>"#,
        )
        .unwrap();
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].tool, "read_file");
    }

    #[test]
    fn parses_fenced_json_tool_call() {
        let parsed = parse_model_output(
            "```json\n{\"tool\":\"write_file\",\"arguments\":{\"path\":\"index.html\",\"content\":\"x\"}}\n```",
        )
        .unwrap();
        assert_eq!(parsed.calls[0].tool, "write_file");
    }

    #[test]
    fn parses_simple_action_json() {
        let parsed = parse_model_output(
            r#"{"action":"write_file","path":"index.html","content":"<html></html>"}"#,
        )
        .unwrap();
        assert_eq!(parsed.calls[0].tool, "write_file");
        assert_eq!(parsed.calls[0].arguments["path"], "index.html");
    }

    #[test]
    fn recovers_partial_xml_json() {
        let parsed = parse_model_output(
            r#"<tool_call>{"tool":"shell","arguments":{"command":"cargo test"}}"#,
        )
        .unwrap();
        assert_eq!(parsed.calls[0].tool, "shell");
    }

    #[test]
    fn recovers_malformed_local_write_file_action() {
        let parsed = parse_model_output(
            "{\"action\":\"write_file\",\"path\":\"index.html\",\"content\":\"<html>\n<style>\n.card {}\n</style>\n</html>\"}",
        )
        .unwrap();
        assert_eq!(parsed.calls[0].tool, "write_file");
        assert_eq!(parsed.calls[0].arguments["path"], "index.html");
        assert!(parsed.calls[0].arguments["content"]
            .as_str()
            .unwrap()
            .contains("<style>"));
    }

    #[test]
    fn recovers_spaced_fenced_malformed_local_write_file_action() {
        let parsed = parse_model_output(
            "```json\n{\"action\":\"write_file\",\"path\":\"index.html\",\"content\": \"<!DOCTYPE html>\\n<html>ok</html>\\n```\n",
        )
        .unwrap();
        assert_eq!(parsed.calls[0].tool, "write_file");
        assert_eq!(parsed.calls[0].arguments["path"], "index.html");
    }
}
