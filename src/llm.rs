use crate::config::AppConfig;
use crate::edits;
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

const SYSTEM_PROMPT: &str = r#"You are Smol CLI, a helpful coding assistant that can analyze codebases and propose safe, minimal file edits.

You have access to tools: read, list, edit, answer.

Tool descriptions:
- read: Read a file. Arguments: {"file_path": "path/to/file"}
- list: List directory contents. Arguments: {"path": "path/to/dir"}
- edit: Edit a file by replacing text. Arguments: {"file_path": "path/to/file", "old_string": "exact text to replace", "new_string": "replacement text"}
- answer: Provide an answer to a question. Arguments: {"text": "answer text"}

For code changes:
- First use read/list to understand the codebase
- Then use edit to make changes with exact old_string/new_string

For questions:
- Use answer to respond
- Use read/list if needed

Output ONLY a JSON array of tool calls. Example:
[{"name": "read", "arguments": {"file_path": "README.md"}}]

Do not output any other text."#;

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
            },
            Message {
                role: "user".to_string(),
                content: format!("Context:\n{}\n\nQuestion: {}", context, user_prompt),
                tool_calls: None,
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

    let choice = resp.choices.first().ok_or_else(|| anyhow::anyhow!("no choices"))?;
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
    let body = ChatRequest {
        model: cfg.provider.model.clone(),
        messages: vec![
            Message {
                role: "system".to_string(),
                content: SYSTEM_PROMPT.to_string(),
                tool_calls: None,
            },
            Message {
                role: "user".to_string(),
                content: format!("Context:\n{}\n\nRequest: {}", context, user_prompt),
                tool_calls: None,
            },
        ],
        temperature: Some(cfg.runtime.temperature),
        tools: None, // No API tools, model outputs JSON
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

    let choice = resp.choices.first().ok_or_else(|| anyhow::anyhow!("no choices"))?;
    let content = choice.message.content.clone();

    // Parse the content as JSON tool calls
    match serde_json::from_str::<Vec<serde_json::Value>>(&content) {
        Ok(tool_calls_json) => {
            let mut edit_actions = Vec::new();
            for tool_call_json in tool_calls_json {
                if let Some(name) = tool_call_json.get("name").and_then(|n| n.as_str()) {
                    if let Some(args) = tool_call_json.get("arguments") {
                        match name {
                            "read" | "list" => {
                                // Execute and ignore result for now
                                let args_str = serde_json::to_string(args).unwrap_or_default();
                                let tool_call = ToolCall {
                                    id: "manual".to_string(),
                                    r#type: "function".to_string(),
                                    function: ToolCallFunction {
                                        name: name.to_string(),
                                        arguments: args_str,
                                    },
                                };
                                execute_tool(repo_root, &tool_call.function).await;
                            }
                            "edit" => {
                                if let (Some(file_path), Some(old_string), Some(new_string)) = (
                                    args.get("file_path").and_then(|v| v.as_str()),
                                    args.get("old_string").and_then(|v| v.as_str()),
                                    args.get("new_string").and_then(|v| v.as_str()),
                                ) {
                                    // For edit, we can apply it directly or collect as action
                                    // Since it's replace, create an edit action
                                    edit_actions.push(edits::Edit {
                                        path: file_path.to_string(),
                                        op: "replace".to_string(),
                                        anchor: old_string.to_string(),
                                        snippet: new_string.to_string(),
                                        limit: 1,
                                        rationale: None,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            // Return the edit actions as JSON
            let batch = edits::EditBatch { edits: edit_actions };
            Ok(EditResponse {
                content: serde_json::to_string(&vec![serde_json::json!({"edits": batch.edits})]).unwrap_or_default(),
                usage: resp.usage,
            })
        }
        Err(_) => {
            // Not JSON, return as is
            Ok(EditResponse {
                content,
                usage: resp.usage,
            })
        }
    }
}

async fn execute_tool(repo_root: &std::path::Path, function: &ToolCallFunction) -> String {
    match function.name.as_str() {
        "read" => {
            match serde_json::from_str::<serde_json::Value>(&function.arguments) {
                Ok(args) => {
                    if let Some(file_path) = args.get("file_path").and_then(|v| v.as_str()) {
                        match crate::fsutil::ensure_inside_repo(repo_root, std::path::Path::new(file_path)) {
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
            }
        }
        "list" => {
            match serde_json::from_str::<serde_json::Value>(&function.arguments) {
                Ok(args) => {
                    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                    let abs_path = if path == "." {
                        repo_root.to_path_buf()
                    } else {
                        match crate::fsutil::ensure_inside_repo(repo_root, std::path::Path::new(path)) {
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
                                        let file_name = entry.file_name().to_string_lossy().to_string();
                                        match entry.file_type() {
                                            Ok(ft) => {
                                                let file_type = if ft.is_dir() { "directory" } else { "file" };
                                                result.push(format!("{} ({})", file_name, file_type));
                                            }
                                            Err(e) => result.push(format!("{} (error: {})", file_name, e)),
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
            }
        }
        "edit" => {
            match serde_json::from_str::<serde_json::Value>(&function.arguments) {
                Ok(args) => {
                    if let (Some(file_path), Some(old_string), Some(new_string)) = (
                        args.get("file_path").and_then(|v| v.as_str()),
                        args.get("old_string").and_then(|v| v.as_str()),
                        args.get("new_string").and_then(|v| v.as_str()),
                    ) {
                        match crate::fsutil::ensure_inside_repo(repo_root, std::path::Path::new(file_path)) {
                            Ok(abs_path) => match std::fs::read_to_string(&abs_path) {
                                Ok(content) => {
                                    if let Some(pos) = content.find(old_string) {
                                        let mut new_content = content.clone();
                                        new_content.replace_range(pos..pos + old_string.len(), new_string);
                                        match std::fs::write(&abs_path, &new_content) {
                                            Ok(_) => format!("Successfully edited {}", file_path),
                                            Err(e) => format!("Error writing file {}: {}", file_path, e),
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
            }
        }
        "answer" => {
            match serde_json::from_str::<serde_json::Value>(&function.arguments) {
                Ok(args) => {
                    args.get("text").and_then(|v| v.as_str()).unwrap_or("No answer provided").to_string()
                }
                Err(e) => format!("Error parsing arguments: {}", e),
            }
        }
        _ => format!("Unknown tool: {}", function.name),
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
        model: cfg.provider.model.clone(),
        messages: vec![
            Message {
                role: "system".to_string(),
                content: PLANNER_PROMPT.to_string(),
                tool_calls: None,
            },
            Message {
                role: "user".to_string(),
                content: user_prompt.to_string(),
                tool_calls: None,
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
