use crate::config::AppConfig;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Deserialize, Debug, Clone)]
pub struct Choice {
    pub message: AssistantMessage,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AssistantMessage {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Usage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub total_cost: Option<f64>,
}

#[derive(Deserialize, Debug)]
struct ChatResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Clone)]
pub struct EditResponse {
    pub content: String,
    pub usage: Option<Usage>,
}

fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Number(num)) => num.as_f64(),
        Some(serde_json::Value::String(s)) => s.parse().ok(),
        _ => None,
    })
}

const SYSTEM_PROMPT: &str = r#"You are Smol CLI, a conservative coding agent that proposes safe, minimal file edits.
Return ONLY JSON with the schema:
{"edits":[{"path":"...", "op":"replace|insert_after|insert_before", "anchor":"...", "snippet":"...", "limit":1, "rationale":"..."}]}
- Use small, anchor-based changes.
- Never return shell commands.
- Keep edits minimal and specific."#;

const PLANNER_PROMPT: &str = r#"You are Smol CLI's planning assistant.
Given a user request, produce a JSON object with the schema:
{"plan":[{"description":"...", "read": "relative/path.ext" | null}]}
- Break the work into 2-5 concise steps.
- Use "read" to request file contents needed for the task (relative to repo root). Use null if reading a file is not required for that step.
- Prefer explicit file paths like "src/lib.rs" when reading files.
- Keep descriptions short and actionable.
Respond with JSON only."#;

pub async fn propose_edits(
    cfg: &AppConfig,
    user_prompt: &str,
    context: &str,
) -> Result<EditResponse> {
    let body = ChatRequest {
        model: &cfg.provider.model,
        messages: vec![
            Message {
                role: "system",
                content: SYSTEM_PROMPT,
            },
            Message {
                role: "user",
                content: context,
            },
            Message {
                role: "user",
                content: user_prompt,
            },
        ],
        temperature: Some(cfg.runtime.temperature),
    };

    let client = Client::new();
    let url = format!(
        "{}/chat/completions",
        cfg.provider.base_url.trim_end_matches('/')
    );
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

    let content = resp
        .choices
        .first()
        .map(|c| {
            debug_assert_eq!(&c.message.role, "assistant");
            c.message.content.clone()
        })
        .unwrap_or_default();

    Ok(EditResponse {
        content,
        usage: resp.usage,
    })
}

pub async fn generate_plan(cfg: &AppConfig, user_prompt: &str) -> Result<String> {
    let body = ChatRequest {
        model: &cfg.provider.model,
        messages: vec![
            Message {
                role: "system",
                content: PLANNER_PROMPT,
            },
            Message {
                role: "user",
                content: user_prompt,
            },
        ],
        temperature: Some(0.0),
    };

    let client = Client::new();
    let url = format!(
        "{}/chat/completions",
        cfg.provider.base_url.trim_end_matches('/')
    );
    let resp: ChatResponse = client
        .post(url)
        .bearer_auth(&cfg.auth.api_key)
        .json(&body)
        .send()
        .await
        .context("plan request failed")?
        .error_for_status()
        .context("plan non-200")?
        .json()
        .await
        .context("plan decode failed")?;

    let content = resp
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    Ok(content)
}

#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub prompt_cost: Option<f64>,
    pub completion_cost: Option<f64>,
    pub context_length: Option<u32>,
}

#[derive(Deserialize, Debug)]
struct ModelsResponse {
    data: Vec<ApiModel>,
}

#[derive(Deserialize, Debug)]
struct ApiModel {
    id: String,
    name: String,
    #[serde(default)]
    pricing: Option<ApiPricing>,
    #[serde(default)]
    context_length: Option<u32>,
    #[serde(default)]
    top_provider: Option<ApiTopProvider>,
}

#[derive(Deserialize, Debug)]
struct ApiPricing {
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    prompt: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    completion: Option<f64>,
}

#[derive(Deserialize, Debug)]
struct ApiTopProvider {
    #[serde(default)]
    context_length: Option<u32>,
}

pub async fn list_models(cfg: &AppConfig) -> Result<Vec<Model>> {
    let client = Client::new();
    let url = format!(
        "{}/models?category=programming",
        cfg.provider.base_url.trim_end_matches('/')
    );
    let resp: ModelsResponse = client
        .get(url)
        .bearer_auth(&cfg.auth.api_key)
        .send()
        .await
        .context("models request failed")?
        .error_for_status()
        .context("models non-200")?
        .json()
        .await
        .context("models decode failed")?;

    let models = resp
        .data
        .into_iter()
        .map(|m| Model {
            id: m.id,
            name: m.name,
            prompt_cost: m.pricing.as_ref().and_then(|p| p.prompt),
            completion_cost: m.pricing.as_ref().and_then(|p| p.completion),
            context_length: m
                .context_length
                .or_else(|| m.top_provider.as_ref()?.context_length),
        })
        .collect();

    Ok(models)
}
