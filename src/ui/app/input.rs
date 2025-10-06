use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::Input;

use crate::{config, llm, ui::app::prompt};

use super::state::{App, MessageKind, ModelPickerState, WELCOME_MSG};

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

    if let (Some(picker), Some(models)) = (app.model_picker.as_mut(), app.models.as_ref()) {
        match key.code {
            KeyCode::Up => {
                if picker.index > 0 {
                    picker.index -= 1;
                }
            }
            KeyCode::Down => {
                if picker.index + 1 < models.len() {
                    picker.index += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(model) = models.get(picker.index) {
                    app.cfg.provider.model = model.id.clone();
                    config::save(&app.cfg)?;
                    app.current_model = Some(model.clone());
                    app.add_message(
                        MessageKind::Info,
                        format!(
                            "Model set to {} ({}) — in {} out {} ctx {}",
                            model.name,
                            model.id,
                            display_cost(model.prompt_cost),
                            display_cost(model.completion_cost),
                            display_ctx(model.context_length)
                        ),
                    );
                }
                app.model_picker = None;
                app.caret_visible = true;
            }
            KeyCode::Esc => {
                app.model_picker = None;
                app.add_message(MessageKind::Info, "Model selection cancelled.".into());
                app.caret_visible = true;
            }
            _ => {}
        }
        if app.model_picker.is_some() {
            app.caret_visible = false;
        }
        return Ok(());
    }

    if key.code == KeyCode::Enter
        && (key.modifiers.contains(KeyModifiers::SHIFT)
            || key.modifiers.contains(KeyModifiers::ALT)
            || key.modifiers.contains(KeyModifiers::CONTROL))
    {
        app.textarea.insert_newline();
        app.caret_visible = true;
        return Ok(());
    }

    if key.code == KeyCode::Char('z') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.undo_last();
        return Ok(());
    }

    // Handle activity scrolling
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('u') | KeyCode::Up => {
                if app.activity_scroll > 0 {
                    app.activity_scroll = app.activity_scroll.saturating_sub(1);
                    app.auto_scroll_enabled = false;
                }
                return Ok(());
            }
            KeyCode::Char('d') | KeyCode::Down => {
                let max_scroll = app.messages.len().saturating_sub(1);
                if app.activity_scroll < max_scroll {
                    app.activity_scroll = app.activity_scroll.saturating_add(1);
                    app.auto_scroll_enabled = false;
                }
                return Ok(());
            }
            KeyCode::Char('b') | KeyCode::PageUp => {
                app.activity_scroll = app.activity_scroll.saturating_sub(10);
                app.auto_scroll_enabled = false;
                return Ok(());
            }
            KeyCode::Char('f') | KeyCode::PageDown => {
                let max_scroll = app.messages.len().saturating_sub(1);
                app.activity_scroll = (app.activity_scroll + 10).min(max_scroll);
                app.auto_scroll_enabled = false;
                return Ok(());
            }
            KeyCode::Home => {
                app.activity_scroll = 0;
                app.auto_scroll_enabled = false;
                return Ok(());
            }
            KeyCode::End => {
                app.activity_scroll = app.messages.len().saturating_sub(1);
                app.auto_scroll_enabled = true;
                return Ok(());
            }
            _ => {}
        }
    }

    if key.code == KeyCode::Enter && key.modifiers.is_empty() {
        app.submit_prompt().await?;
        return Ok(());
    }

    let input = Input::from(Event::Key(key));
    app.textarea.input(input);
    Ok(())
}

fn display_cost(cost: Option<f64>) -> String {
    cost.map(|c| format!("${:.2}/M", c * 1_000.0))
        .unwrap_or_else(|| "--".into())
}

fn display_ctx(ctx: Option<u32>) -> String {
    ctx.map(format_ctx_value).unwrap_or_else(|| "--".into())
}

fn format_ctx_value(c: u32) -> String {
    if c % 1000 == 0 {
        format!("{}K", c / 1000)
    } else {
        format!("{:.1}K", c as f32 / 1000.0)
    }
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
            app.memory.clear();
            app.total_tokens_used = 0;
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
                        let count = models.len();
                        if count == 0 {
                            app.add_message(
                                MessageKind::Warn,
                                "No programming models available.".into(),
                            );
                            app.models = Some(models);
                            app.current_model = None;
                            app.model_picker = None;
                        } else {
                            app.add_message(
                                MessageKind::Info,
                                format!("Loaded {count} programming models."),
                            );
                            app.add_message(
                                MessageKind::Info,
                                "Use ↑/↓ to choose, Enter to confirm, Esc to cancel.".into(),
                            );
                            let current = models
                                .iter()
                                .find(|m| m.id == app.cfg.provider.model)
                                .cloned();
                            app.models = Some(models);
                            app.current_model = current;
                            app.model_picker = Some(ModelPickerState { index: 0 });
                            app.caret_visible = false;
                        }
                    }
                    Err(e) => {
                        app.add_message(
                            MessageKind::Error,
                            format!("Failed to fetch models: {}", e),
                        );
                        app.model_picker = None;
                    }
                }
            } else if parts.len() == 2 {
                if let Ok(n) = parts[1].parse::<usize>() {
                    if let Some(models) = &app.models {
                        if n > 0 && n <= models.len() {
                            let model = &models[n - 1];
                            app.cfg.provider.model = model.id.clone();
                            config::save(&app.cfg)?;
                            app.current_model = Some(model.clone());
                            app.add_message(
                                MessageKind::Info,
                                format!(
                                    "Model set to {} ({}) — in {} out {} ctx {}",
                                    model.name,
                                    model.id,
                                    display_cost(model.prompt_cost),
                                    display_cost(model.completion_cost),
                                    display_ctx(model.context_length)
                                ),
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
                    app.current_model = None;
                    app.add_message(
                        MessageKind::Info,
                        format!("Model set to {}", app.cfg.provider.model),
                    );
                }
                app.model_picker = None;
                app.caret_visible = true;
            } else {
                app.add_message(
                    MessageKind::Warn,
                    "Usage: /model [<number> | <provider/model>], e.g., grok-4-fast:free".into(),
                );
                app.current_model = None;
                app.model_picker = None;
                app.caret_visible = true;
            }
        }
        _ => app.add_message(MessageKind::Warn, "Unknown command. /help".into()),
    }
    Ok(())
}
