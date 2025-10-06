use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Edit {
    pub path: String,
    pub op: String, // "replace" | "insert_after" | "insert_before"
    pub anchor: String,
    pub snippet: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EditBatch {
    pub edits: Vec<Edit>,
}

fn default_limit() -> usize {
    1
}

#[derive(Debug, Clone)]
pub enum Action {
    Edit(Edit),
    ReadFile { path: String },
    ListDirectory { path: String },
    ProvideAnswer { answer: String },
}

pub fn parse_actions(json_text: &str) -> Result<Vec<Action>> {
    let tool_calls: Vec<serde_json::Value> = serde_json::from_str(json_text)
        .context("failed to parse tool calls")?;

    let actions = tool_calls
        .into_iter()
        .filter_map(|call| {
            let function = call.get("function")?;
            let name = function.get("name")?.as_str()?;
            let args: serde_json::Value = serde_json::from_str(function.get("arguments")?.as_str()?).ok()?;

            match name {
                "read" => {
                    let path = args.get("file_path")?.as_str()?.to_string();
                    Some(Action::ReadFile { path })
                }
                "list" => {
                    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".").to_string();
                    Some(Action::ListDirectory { path })
                }
                "edit" => {
                    let path = args.get("file_path")?.as_str()?.to_string();
                    let old_string = args.get("old_string")?.as_str()?.to_string();
                    let new_string = args.get("new_string")?.as_str()?.to_string();
                    Some(Action::Edit(Edit {
                        path,
                        op: "replace".to_string(),
                        anchor: old_string,
                        snippet: new_string,
                        limit: 1,
                        rationale: None,
                    }))
                }
                "answer" => {
                    let text = args.get("text")?.as_str()?.to_string();
                    Some(Action::ProvideAnswer { answer: text })
                }
                _ => None,
            }
        })
        .collect();

    Ok(actions)
}

pub fn parse_edits(json_text: &str) -> Result<EditBatch> {
    let actions = parse_actions(json_text)?;
    let edits = actions
        .into_iter()
        .filter_map(|action| {
            match action {
                Action::Edit(edit) => Some(edit),
                _ => None,
            }
        })
        .collect();

    Ok(EditBatch { edits })
}

pub fn apply_edit(original: &str, e: &Edit) -> Result<String> {
    match e.op.as_str() {
        "replace" => replace_once(original, &e.anchor, &e.snippet, e.limit),
        "insert_after" => insert_after(original, &e.anchor, &e.snippet),
        "insert_before" => insert_before(original, &e.anchor, &e.snippet),
        other => Err(anyhow::anyhow!("unsupported op: {other}")),
    }
}

fn replace_once(s: &str, anchor: &str, snippet: &str, limit: usize) -> Result<String> {
    let count = s.matches(anchor).count();
    if count < limit {
        anyhow::bail!("anchor not found enough times");
    }
    // replace first (limit=1 for v0)
    let mut parts = s.splitn(2, anchor);
    let head = parts.next().unwrap_or("");
    let tail = parts.next().unwrap_or("");
    Ok(format!("{head}{snippet}{tail}"))
}

fn insert_after(s: &str, anchor: &str, snippet: &str) -> Result<String> {
    if let Some(idx) = s.find(anchor) {
        let insert_at = idx + anchor.len();
        let mut out = String::with_capacity(s.len() + snippet.len());
        out.push_str(&s[..insert_at]);
        out.push_str(snippet);
        out.push_str(&s[insert_at..]);
        Ok(out)
    } else {
        anyhow::bail!("anchor not found");
    }
}

fn insert_before(s: &str, anchor: &str, snippet: &str) -> Result<String> {
    if let Some(idx) = s.find(anchor) {
        let mut out = String::with_capacity(s.len() + snippet.len());
        out.push_str(&s[..idx]);
        out.push_str(snippet);
        out.push_str(&s[idx..]);
        Ok(out)
    } else {
        anyhow::bail!("anchor not found");
    }
}
