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

pub fn parse_edits(json_text: &str) -> Result<EditBatch> {
    // try plain JSON; if model wrapped in markdown, strip code fences
    let trimmed = json_text.trim().trim_matches('`').trim();
    let batch: EditBatch = serde_json::from_str(trimmed)
        .or_else(|_| serde_json::from_str(json_text))
        .context("failed to parse edits JSON")?;
    Ok(batch)
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
