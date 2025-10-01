use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use crate::config::AppConfig;

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Deserialize, Debug)]
pub struct Choice {
    pub message: AssistantMessage,
}

#[derive(Deserialize, Debug)]
pub struct AssistantMessage {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize, Debug)]
struct ChatResponse {
    choices: Vec<Choice>,
}

const SYSTEM_PROMPT: &str = r#"You are Smol CLI, a conservative coding agent that proposes safe, minimal file edits.
Return ONLY JSON with the schema:
{"edits":[{"path":"...", "op":"replace|insert_after|insert_before", "anchor":"...", "snippet":"...", "limit":1, "rationale":"..."}]}
- Use small, anchor-based changes.
- Never return shell commands.
- Keep edits minimal and specific."#;

pub async fn propose_edits(cfg: &AppConfig, user_prompt: &str, context: &str) -> Result<String> {
    let body = ChatRequest {
        model: &cfg.provider.model,
        messages: vec![
            Message { role: "system", content: SYSTEM_PROMPT },
            Message { role: "user", content: context },
            Message { role: "user", content: user_prompt },
        ],
        temperature: Some(cfg.runtime.temperature),
    };

    let client = Client::new();
    let url = format!("{}/chat/completions", cfg.provider.base_url.trim_end_matches('/'));
    let resp: ChatResponse = client
        .post(url)
        .bearer_auth(&cfg.auth.api_key)
        .json(&body)
        .send()
        .await
        .context("llm request failed")?
        .error_for_status()
        .context("llm non-200")?
        .json()
        .await
        .context("llm decode failed")?;

    let content = resp.choices.first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    Ok(content)
}
