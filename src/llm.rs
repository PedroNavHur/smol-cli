use crate::config::AppConfig;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Clone)]
struct Message {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
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

#[derive(Serialize, Clone)]
struct Tool {
    r#type: String, // "function"
    function: ToolFunction,
}

#[derive(Serialize, Clone)]
struct ToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value, // JSON schema
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Tool>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Choice {
    pub message: AssistantMessage,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AssistantMessage {
    #[allow(dead_code)]
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

const SYSTEM_PROMPT: &str = r#"You are Smol CLI, a coding assistant that proposes safe file edits.

You have access to tools: read, list, edit.

To propose code changes:
- Use read or list to understand the current codebase
- Use edit to propose exact changes with file_path, old_string, and new_string

Always use the edit tool for code modifications. Do not describe changes in text."#;

const INFO_SYSTEM_PROMPT: &str = r#"You are Smol CLI, answering questions about codebases.

Context has been gathered from exploring the codebase. Now provide a helpful answer to: {user_question}

MANDATORY: Use ONLY the answer tool for your response. Never output text directly.

Tools:
- answer: Your response (MUST USE THIS)

Do not use other tools unless absolutely necessary. Always use answer with your complete answer."#;

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

const MAX_TOOL_OUTPUT_CHARS: usize = 16_000;

fn edit_tools() -> Vec<Tool> {
    vec![
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Path to the file to read"}
                    },
                    "required": ["file_path"]
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "list".to_string(),
                description: "List directory contents".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Path to the directory to list", "default": "."}
                    }
                }),
            },
        },
        Tool {
            r#type: "function".to_string(),
            function: ToolFunction {
                name: "edit".to_string(),
                description: "Edit a file by replacing text".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string", "description": "Path to the file to modify"},
                        "old_string": {"type": "string", "description": "Exact text to replace"},
                        "new_string": {"type": "string", "description": "Text to replace it with"}
                    },
                    "required": ["file_path", "old_string", "new_string"]
                }),
            },
        },
    ]
}

pub async fn provide_information(
    cfg: &AppConfig,
    repo_root: &std::path::Path,
    user_prompt: &str,
    context: &str,
) -> Result<EditResponse> {
    let system_prompt = INFO_SYSTEM_PROMPT.replace("{user_question}", user_prompt);
    let body = ChatRequest {
        model: cfg.provider.model.clone(),
        messages: vec![
            Message {
                role: "system".to_string(),
                content: system_prompt,
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: "user".to_string(),
                content: format!("Context:\n{}\n\nQuestion: {}", context, user_prompt),
                tool_calls: None,
                tool_call_id: None,
            },
        ],
        temperature: Some(cfg.runtime.temperature),
        tools: None,
    };

    let client = Client::new();
    let url = format!(
        "{}/chat/completions",
        cfg.provider.base_url.trim_end_matches('/')
    );

    let resp: ChatResponse = client
        .post(&url)
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

    let choice = resp
        .choices
        .first()
        .ok_or_else(|| anyhow::anyhow!("no choices"))?;
    let content = choice.message.content.clone();

    // Parse as answer tool call
    if let Ok(tool_call) = serde_json::from_str::<serde_json::Value>(&content) {
        if let Some(name) = tool_call.get("name").and_then(|n| n.as_str()) {
            if name == "answer" {
                if let Some(args) = tool_call.get("arguments") {
                    if let Some(text) = args.get("text").and_then(|t| t.as_str()) {
                        return Ok(EditResponse {
                            content: text.to_string(),
                            usage: resp.usage,
                        });
                    }
                }
            }
        }
    }

    // Return as is
    Ok(EditResponse {
        content,
        usage: resp.usage,
    })
}

pub async fn propose_edits(
    cfg: &AppConfig,
    repo_root: &std::path::Path,
    user_prompt: &str,
    context: &str,
) -> Result<EditResponse> {
    let tools = edit_tools();
    let client = Client::new();
    let url = format!(
        "{}/chat/completions",
        cfg.provider.base_url.trim_end_matches('/')
    );
    let mut messages = vec![
        Message {
            role: "system".to_string(),
            content: SYSTEM_PROMPT.to_string(),
            tool_calls: None,
            tool_call_id: None,
        },
        Message {
            role: "user".to_string(),
            content: format!("Context:\n{}\n\nRequest: {}", context, user_prompt),
            tool_calls: None,
            tool_call_id: None,
        },
    ];

    let mut total_usage: Option<Usage> = None;

    for _ in 0..6 {
        let body = ChatRequest {
            model: cfg.provider.model.clone(),
            messages: messages.clone(),
            temperature: Some(cfg.runtime.temperature),
            tools: Some(tools.clone()),
        };

        let resp: ChatResponse = client
            .post(&url)
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

        total_usage = merge_usage(total_usage, resp.usage.clone());

        let choice = resp
            .choices
            .first()
            .ok_or_else(|| anyhow::anyhow!("no choices"))?;
        let assistant_message = choice.message.clone();

        if assistant_message.tool_calls.is_empty() {
            return Ok(EditResponse {
                content: assistant_message.content,
                usage: total_usage,
            });
        }

        messages.push(Message {
            role: "assistant".to_string(),
            content: assistant_message.content.clone(),
            tool_calls: Some(assistant_message.tool_calls.clone()),
            tool_call_id: None,
        });

        let mut edit_calls = Vec::new();

        for tool_call in &assistant_message.tool_calls {
            match tool_call.function.name.as_str() {
                "read" | "list" => {
                    let output = execute_tool(repo_root, &tool_call.function).await;
                    messages.push(Message {
                        role: "tool".to_string(),
                        content: output,
                        tool_calls: None,
                        tool_call_id: Some(tool_call.id.clone()),
                    });
                }
                "edit" => {
                    edit_calls.push(tool_call.clone());
                }
                other => {
                    let output = format!("Unsupported tool call: {}", other);
                    messages.push(Message {
                        role: "tool".to_string(),
                        content: output,
                        tool_calls: None,
                        tool_call_id: Some(tool_call.id.clone()),
                    });
                }
            }
        }

        if !edit_calls.is_empty() {
            return Ok(EditResponse {
                content: serde_json::to_string(&edit_calls).unwrap_or_default(),
                usage: total_usage,
            });
        }
    }

    Err(anyhow::anyhow!("LLM did not produce edits"))
}

async fn execute_tool(repo_root: &std::path::Path, function: &ToolCallFunction) -> String {
    match function.name.as_str() {
        "read" => {
            let output = match serde_json::from_str::<serde_json::Value>(&function.arguments) {
                Ok(args) => {
                    if let Some(file_path) = args.get("file_path").and_then(|v| v.as_str()) {
                        match crate::fsutil::ensure_inside_repo(
                            repo_root,
                            std::path::Path::new(file_path),
                        ) {
                            Ok(abs_path) => match std::fs::read_to_string(&abs_path) {
                                Ok(content) => content,
                                Err(e) => format!("Error reading file {}: {}", file_path, e),
                            },
                            Err(e) => format!("Invalid path {}: {}", file_path, e),
                        }
                    } else {
                        "Error: missing file_path argument".to_string()
                    }
                }
                Err(e) => format!("Error parsing arguments: {}", e),
            };
            truncate_output(output)
        }
        "list" => {
            let output = match serde_json::from_str::<serde_json::Value>(&function.arguments) {
                Ok(args) => {
                    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                    let abs_path = if path == "." {
                        repo_root.to_path_buf()
                    } else {
                        match crate::fsutil::ensure_inside_repo(
                            repo_root,
                            std::path::Path::new(path),
                        ) {
                            Ok(p) => p,
                            Err(e) => return format!("Invalid path {}: {}", path, e),
                        }
                    };
                    match std::fs::read_dir(&abs_path) {
                        Ok(entries) => {
                            let mut result = Vec::new();
                            for entry in entries {
                                match entry {
                                    Ok(entry) => {
                                        let file_name =
                                            entry.file_name().to_string_lossy().to_string();
                                        match entry.file_type() {
                                            Ok(ft) => {
                                                let file_type =
                                                    if ft.is_dir() { "directory" } else { "file" };
                                                result
                                                    .push(format!("{} ({})", file_name, file_type));
                                            }
                                            Err(e) => {
                                                result.push(format!("{} (error: {})", file_name, e))
                                            }
                                        }
                                    }
                                    Err(e) => result.push(format!("Error reading entry: {}", e)),
                                }
                            }
                            result.sort();
                            result.join("\n")
                        }
                        Err(e) => format!("Error listing directory {}: {}", path, e),
                    }
                }
                Err(e) => format!("Error parsing arguments: {}", e),
            };
            truncate_output(output)
        }
        "edit" => match serde_json::from_str::<serde_json::Value>(&function.arguments) {
            Ok(args) => {
                if let (Some(file_path), Some(old_string), Some(new_string)) = (
                    args.get("file_path").and_then(|v| v.as_str()),
                    args.get("old_string").and_then(|v| v.as_str()),
                    args.get("new_string").and_then(|v| v.as_str()),
                ) {
                    match crate::fsutil::ensure_inside_repo(
                        repo_root,
                        std::path::Path::new(file_path),
                    ) {
                        Ok(abs_path) => match std::fs::read_to_string(&abs_path) {
                            Ok(content) => {
                                if let Some(pos) = content.find(old_string) {
                                    let mut new_content = content.clone();
                                    new_content
                                        .replace_range(pos..pos + old_string.len(), new_string);
                                    match std::fs::write(&abs_path, &new_content) {
                                        Ok(_) => format!("Successfully edited {}", file_path),
                                        Err(e) => {
                                            format!("Error writing file {}: {}", file_path, e)
                                        }
                                    }
                                } else {
                                    format!("old_string not found in {}", file_path)
                                }
                            }
                            Err(e) => format!("Error reading file {}: {}", file_path, e),
                        },
                        Err(e) => format!("Invalid path {}: {}", file_path, e),
                    }
                } else {
                    "Error: missing file_path, old_string, or new_string argument".to_string()
                }
            }
            Err(e) => format!("Error parsing arguments: {}", e),
        },
        "answer" => match serde_json::from_str::<serde_json::Value>(&function.arguments) {
            Ok(args) => args
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("No answer provided")
                .to_string(),
            Err(e) => format!("Error parsing arguments: {}", e),
        },
        _ => format!("Unknown tool: {}", function.name),
    }
}

fn truncate_output(text: String) -> String {
    if text.chars().count() <= MAX_TOOL_OUTPUT_CHARS {
        return text;
    }

    let truncated: String = text.chars().take(MAX_TOOL_OUTPUT_CHARS).collect();
    format!("{}\n... [truncated]", truncated)
}

fn merge_usage(existing: Option<Usage>, new: Option<Usage>) -> Option<Usage> {
    match (existing, new) {
        (None, None) => None,
        (Some(u), None) => Some(u),
        (None, Some(u)) => Some(u),
        (Some(mut acc), Some(u)) => {
            acc.prompt_tokens = sum_option_u32(acc.prompt_tokens, u.prompt_tokens);
            acc.completion_tokens = sum_option_u32(acc.completion_tokens, u.completion_tokens);
            acc.total_tokens = sum_option_u32(acc.total_tokens, u.total_tokens);
            acc.total_cost = sum_option_f64(acc.total_cost, u.total_cost);
            Some(acc)
        }
    }
}

fn sum_option_u32(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

fn sum_option_f64(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
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
                description: "Plan to answer an informational question about the codebase"
                    .to_string(),
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
        model: cfg.provider.model.clone(),
        messages: vec![
            Message {
                role: "system".to_string(),
                content: PLANNER_PROMPT.to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: "user".to_string(),
                content: user_prompt.to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
        ],
        temperature: Some(0.0),
        tools: Some(tools),
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
