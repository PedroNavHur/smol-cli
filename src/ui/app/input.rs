use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::Input;

use crate::{config, llm, ui::app::prompt};

use super::state::{App, MessageKind, WELCOME_MSG};

pub(super) async fn on_key(app: &mut App, key: KeyEvent) -> Result<()> {
    app.caret_visible = true;
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return Ok(());
    }

    if key.code == KeyCode::Tab && key.modifiers.is_empty() {
        if prompt::try_accept_suggestion(app) {
            return Ok(());
        }
    }

    if app.review.is_some() {
        match key.code {
            KeyCode::Char('y') => {
                if let Err(err) = app.apply_current() {
                    app.add_message(MessageKind::Error, format!("Apply failed: {err}"));
                }
            }
            KeyCode::Char('n') => {
                app.skip_current("Skipped by user");
            }
            KeyCode::Char('b') => {
                app.review = None;
                app.add_message(MessageKind::Info, "Exited review.".into());
                app.caret_visible = true;
            }
            _ => {}
        }
        return Ok(());
    }

    if key.code == KeyCode::Char('z') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.undo_last();
        return Ok(());
    }

    if key.code == KeyCode::Enter && key.modifiers.is_empty() {
        app.submit_prompt().await?;
        return Ok(());
    }

    let input = Input::from(Event::Key(key));
    app.textarea.input(input);
    Ok(())
}

pub(super) fn on_paste(app: &mut App, data: String) {
    if app.review.is_none() {
        app.textarea.insert_str(&data);
        app.caret_visible = true;
    }
}

pub(super) async fn handle_command(app: &mut App, input: &str) -> Result<()> {
    app.caret_visible = true;
    match input {
        "/help" => app.add_message(
            MessageKind::Info,
            "/login  /model  /clear  /undo  /stats  /quit".into(),
        ),
        "/quit" | "/exit" => {
            app.should_quit = true;
        }
        "/clear" => {
            app.messages.clear();
            app.history.clear();
            app.add_message(MessageKind::Info, "History cleared.".into());
            app.add_message(MessageKind::Info, WELCOME_MSG.into());
        }
        "/stats" => {
            app.add_message(
                MessageKind::Info,
                format!("Messages: {}", app.history.len()),
            );
        }
        "/undo" => app.undo_last(),
        "/login" => app.add_message(
            MessageKind::Warn,
            "Temporarily unsupported here. Run `/login` in classic chat mode.".into(),
        ),
        cmd if cmd.starts_with("/model") => {
            let parts: Vec<_> = cmd.split_whitespace().collect();
            if parts.len() == 1 {
                app.add_message(MessageKind::Info, "Fetching models...".into());
                match llm::list_models(&app.cfg).await {
                    Ok(models) => {
                        app.add_message(MessageKind::Info, "Available models:".into());
                        for (i, model) in models.iter().enumerate() {
                            app.add_message(
                                MessageKind::Info,
                                format!("{}: {} ({})", i + 1, model.name, model.id),
                            );
                        }
                        app.add_message(
                            MessageKind::Info,
                            "Select a model with /model <number>".into(),
                        );
                        app.models = Some(models);
                    }
                    Err(e) => {
                        app.add_message(
                            MessageKind::Error,
                            format!("Failed to fetch models: {}", e),
                        );
                    }
                }
            } else if parts.len() == 2 {
                if let Ok(n) = parts[1].parse::<usize>() {
                    if let Some(models) = &app.models {
                        if n > 0 && n <= models.len() {
                            let model = &models[n - 1];
                            app.cfg.provider.model = model.id.clone();
                            config::save(&app.cfg)?;
                            app.add_message(
                                MessageKind::Info,
                                format!("Model set to {} ({})", model.name, model.id),
                            );
                        } else {
                            app.add_message(MessageKind::Error, "Invalid model number".into());
                        }
                    } else {
                        app.add_message(
                            MessageKind::Info,
                            "Please run /model first to see the list of models.".into(),
                        );
                    }
                } else {
                    app.cfg.provider.model = parts[1].to_string();
                    config::save(&app.cfg)?;
                    app.add_message(
                        MessageKind::Info,
                        format!("Model set to {}", app.cfg.provider.model),
                    );
                }
            } else {
                app.add_message(
                    MessageKind::Warn,
                    "Usage: /model [<number> | <provider/model>], e.g., grok-4-fast:free".into(),
                );
            }
        }
        _ => app.add_message(MessageKind::Warn, "Unknown command. /help".into()),
    }
    Ok(())
}
