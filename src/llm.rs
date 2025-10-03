use crate::config::AppConfig;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<&'a [ToolCall]>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub r#type: String, // "function"
    pub function: ToolCallFunction,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String, // JSON string
}

#[derive(Serialize)]
struct Tool {
    r#type: String, // "function"
    function: ToolFunction,
}

#[derive(Serialize)]
struct ToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value, // JSON schema
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [Tool]>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Choice {
    pub message: AssistantMessage,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AssistantMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
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

const SYSTEM_PROMPT: &str = r#"You are Smol CLI, a helpful coding assistant that can analyze codebases and propose safe, minimal file edits.
Use the available tools to either:
1. Propose edits when the user wants code changes
2. Provide informational answers when the user asks questions about the codebase
3. Explore the codebase using read_file and list_directory tools

For code changes:
- Use small, anchor-based changes with replace_text, insert_after, insert_before
- Never return shell commands
- Keep edits minimal and specific
- Always provide rationale

For informational queries:
- Use provide_answer to give direct responses
- Use read_file and list_directory to gather information first if needed

Always use the appropriate tools - do not return JSON or text directly."#;

const INFO_SYSTEM_PROMPT: &str = r#"You are Smol CLI, answering questions about codebases.

Context has been gathered from exploring the codebase. Now provide a helpful answer to: {user_question}

MANDATORY: Use ONLY the provide_answer tool for your response. Never output text directly.

Tools:
- provide_answer: Your response (MUST USE THIS)

Do not use other tools unless absolutely necessary. Always provide_answer with your complete answer."#;

const PLANNER_PROMPT: &str = r#"You are Smol CLI's planning assistant.
Given a user request, determine if it's asking for code changes or information about the codebase.

For code changes:
- Break into 2-5 logical steps using read_file, create_file, analyze_code, search_files, list_directory
- Focus on understanding the codebase first, then making changes

For informational queries (like "tell me about this codebase"):
- Start with list_directory to see the project structure
- Then read important files like README.md, main source files, or configuration files
- Use answer_question as the final step to provide the answer

Common files to check: README.md, main.rs, lib.rs, Cargo.toml, package.json, etc.
Be specific about file paths and provide clear reasons for each step."#;

fn edit_tools() -> Vec<Tool> {
    vec![
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "read_file".to_string(),
                description: "Read a specific file to understand the codebase".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Relative path to the file to read"}
                    },
                    "required": ["path"]
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "list_directory".to_string(),
                description: "List contents of a directory".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Relative path to the directory to list", "default": "."}
                    }
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "replace_text".to_string(),
                description: "Replace text in a file using an anchor".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Relative path to the file"},
                        "anchor": {"type": "string", "description": "Unique text to anchor the replacement"},
                        "snippet": {"type": "string", "description": "New text to replace with"},
                        "rationale": {"type": "string", "description": "Reason for this edit"}
                    },
                    "required": ["path", "anchor", "snippet"]
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "insert_after".to_string(),
                description: "Insert text after an anchor in a file".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Relative path to the file"},
                        "anchor": {"type": "string", "description": "Text to insert after"},
                        "snippet": {"type": "string", "description": "Text to insert"},
                        "rationale": {"type": "string", "description": "Reason for this edit"}
                    },
                    "required": ["path", "anchor", "snippet"]
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "insert_before".to_string(),
                description: "Insert text before an anchor in a file".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Relative path to the file"},
                        "anchor": {"type": "string", "description": "Text to insert before"},
                        "snippet": {"type": "string", "description": "Text to insert"},
                        "rationale": {"type": "string", "description": "Reason for this edit"}
                    },
                    "required": ["path", "anchor", "snippet"]
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "provide_answer".to_string(),
                description: "Provide an informational answer about the codebase".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "answer": {"type": "string", "description": "The answer to provide"}
                    },
                    "required": ["answer"]
                }),
            },
        },
    ]
}

pub async fn provide_information(
    cfg: &AppConfig,
    user_prompt: &str,
    context: &str,
) -> Result<EditResponse> {
    let tools = edit_tools();
    let system_prompt = INFO_SYSTEM_PROMPT.replace("{user_question}", user_prompt);
    let context_message = format!("Gathered context:\n{}", context);
    let body = ChatRequest {
        model: &cfg.provider.model,
        messages: vec![
            Message {
                role: "system",
                content: &system_prompt,
                tool_calls: None,
            },
            Message {
                role: "user",
                content: &context_message,
                tool_calls: None,
            },
        ],
        temperature: Some(cfg.runtime.temperature),
        tools: Some(&tools),
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

    let tool_calls = resp
        .choices
        .first()
        .map(|c| c.message.tool_calls.clone())
        .unwrap_or_default();

    // Return tool calls as content for now
    Ok(EditResponse {
        content: serde_json::to_string(&tool_calls).unwrap_or_default(),
        usage: resp.usage,
    })
}

pub async fn propose_edits(
    cfg: &AppConfig,
    user_prompt: &str,
    context: &str,
) -> Result<EditResponse> {
    let tools = edit_tools();
    let body = ChatRequest {
        model: &cfg.provider.model,
        messages: vec![
            Message {
                role: "system",
                content: SYSTEM_PROMPT,
                tool_calls: None,
            },
            Message {
                role: "user",
                content: context,
                tool_calls: None,
            },
            Message {
                role: "user",
                content: user_prompt,
                tool_calls: None,
            },
        ],
        temperature: Some(cfg.runtime.temperature),
        tools: Some(&tools),
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

    let tool_calls = resp
        .choices
        .first()
        .map(|c| c.message.tool_calls.clone())
        .unwrap_or_default();

    // Return tool calls as content for now
    Ok(EditResponse {
        content: serde_json::to_string(&tool_calls).unwrap_or_default(),
        usage: resp.usage,
    })
}

fn plan_tools() -> Vec<Tool> {
    vec![
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "read_file".to_string(),
                description: "Plan to read a specific file to understand the codebase".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Relative path to the file to read"},
                        "reason": {"type": "string", "description": "Why this file needs to be read"}
                    },
                    "required": ["path", "reason"]
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "create_file".to_string(),
                description: "Plan to create a new file".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Relative path for the new file"},
                        "reason": {"type": "string", "description": "Why this file needs to be created"}
                    },
                    "required": ["path", "reason"]
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "list_directory".to_string(),
                description: "Plan to list contents of a directory".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Relative path to the directory to list", "default": "."},
                        "reason": {"type": "string", "description": "Why this directory needs to be listed"}
                    },
                    "required": ["reason"]
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "analyze_code".to_string(),
                description: "Plan to analyze existing code structure".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "focus": {"type": "string", "description": "What aspect to analyze (e.g., 'main function', 'error handling')"},
                        "reason": {"type": "string", "description": "Why this analysis is needed"}
                    },
                    "required": ["focus", "reason"]
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "search_files".to_string(),
                description: "Plan to search for specific patterns or files".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Pattern to search for"},
                        "reason": {"type": "string", "description": "Why this search is needed"}
                    },
                    "required": ["pattern", "reason"]
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "answer_question".to_string(),
                description: "Plan to answer an informational question about the codebase".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "question": {"type": "string", "description": "The question to answer"},
                        "reason": {"type": "string", "description": "Why this question needs to be answered"}
                    },
                    "required": ["question", "reason"]
                }),
            },
        },
    ]
}

pub async fn generate_plan(cfg: &AppConfig, user_prompt: &str) -> Result<String> {
    let tools = plan_tools();
    let body = ChatRequest {
        model: &cfg.provider.model,
        messages: vec![
            Message {
                role: "system",
                content: PLANNER_PROMPT,
                tool_calls: None,
            },
            Message {
                role: "user",
                content: user_prompt,
                tool_calls: None,
            },
        ],
        temperature: Some(0.0),
        tools: Some(&tools),
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

    let tool_calls = resp
        .choices
        .first()
        .map(|c| c.message.tool_calls.clone())
        .unwrap_or_default();

    // For now, return as JSON string for compatibility
    Ok(serde_json::to_string(&tool_calls).unwrap_or_default())
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
