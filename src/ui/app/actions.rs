use anyhow::Result;
use std::path::PathBuf;
use tokio::spawn;

use crate::{agent, config, edits};

use super::state::{App, AsyncEvent, MessageKind};

pub(super) async fn submit_prompt(app: &mut App) -> Result<()> {
    if app.awaiting_response {
        app.add_message(
            MessageKind::Warn,
            "Still waiting for the last response...".into(),
        );
        return Ok(());
    }

    let prompt = app.textarea.lines().join("\n");
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    if trimmed.starts_with('/') {
        app.reset_input();
        super::input::handle_command(app, trimmed).await?;
        return Ok(());
    }

    if app.cfg.auth.api_key.is_empty() {
        app.add_message(
            MessageKind::Error,
            "Missing OpenRouter API key. Set OPENROUTER_API_KEY or use plain mode /login.".into(),
        );
        app.reset_input();
        return Ok(());
    }

    app.add_message(MessageKind::User, trimmed.to_string());
    app.history.push(trimmed.to_string());
    app.reset_input();
    app.awaiting_response = true;
    app.caret_visible = true;

    let cfg = app.cfg.clone();
    let tx = app.tx.clone();
    let prompt = trimmed.to_string();
    let repo_root = app.repo_root.clone();
    let memory = app.memory.clone();

    spawn(async move {
        let event = async_handle_prompt(cfg, repo_root, prompt, memory).await;
        let _ = tx.send(event);
    });

    Ok(())
}

async fn async_handle_prompt(
    cfg: config::AppConfig,
    repo_root: PathBuf,
    prompt: String,
    memory: Vec<String>,
) -> AsyncEvent {
    let context = super::state::build_context(&memory).unwrap_or_else(|_| String::new());
    match agent::run(&cfg, &repo_root, &prompt, context).await {
        Ok(outcome) => match edits::parse_edits(&outcome.response.content) {
            Ok(batch) => AsyncEvent::Edits {
                prompt,
                batch,
                outcome,
            },
            Err(err) => AsyncEvent::ParseError {
                error: err.to_string(),
                raw: outcome.response.content.clone(),
                prompt,
                outcome,
            },
        },
        Err(err) => AsyncEvent::Error(err.to_string()),
    }
}
