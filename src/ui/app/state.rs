use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::{Frame, style::Style};
use tokio::sync::mpsc::UnboundedSender;
use tui_textarea::TextArea;

use super::review::{PreparedEdit, ReviewState};
use crate::{agent, config, diff, edits, fsutil, llm, ui::theme::PROMPT_TEXT};

pub(super) const WELCOME_MSG: &str =
    "Smol CLI — TUI chat. Enter prompts below. y/apply, n/skip during review.";

pub(super) const COMMANDS: &[&str] = &[
    "/help", "/login", "/model", "/clear", "/stats", "/undo", "/quit", "/exit",
];

pub struct App {
    pub(super) cfg: config::AppConfig,
    pub(super) repo_root: PathBuf,
    pub(super) tx: UnboundedSender<AsyncEvent>,
    pub(super) textarea: TextArea<'static>,
    pub(super) messages: Vec<Message>,
    pub(super) view_offset: (u16, u16),
    pub(super) activity_scroll: usize,
    pub(super) completed_steps: Vec<bool>,
    pub(super) history: Vec<String>,
    pub(super) awaiting_response: bool,
    pub(super) review: Option<ReviewState>,
    pub(super) last_backups: Vec<PathBuf>,
    pub(super) should_quit: bool,
    pub(super) caret_visible: bool,
    pub(super) models: Option<Vec<llm::Model>>,
    pub(super) model_picker: Option<ModelPickerState>,
    pub(super) last_usage: Option<llm::Usage>,
    pub(super) current_model: Option<llm::Model>,
    pub(super) memory: Vec<String>,
    pub(super) total_tokens_used: u64,
}

impl App {
    pub(crate) fn new(
        cfg: config::AppConfig,
        repo_root: PathBuf,
        tx: UnboundedSender<AsyncEvent>,
    ) -> Self {
        let mut app = Self {
            cfg,
            repo_root,
            tx,
            textarea: build_textarea(),
            messages: Vec::new(),
            view_offset: (0, 0),
            activity_scroll: 0,
            completed_steps: Vec::new(),
            history: Vec::new(),
            awaiting_response: false,
            review: None,
            last_backups: Vec::new(),
            should_quit: false,
            caret_visible: true,
            models: None,
            model_picker: None,
            last_usage: None,
            current_model: None,
            memory: Vec::new(),
            total_tokens_used: 0,
        };

        if app.cfg.auth.api_key.is_empty() {
            app.add_message(
                MessageKind::Warn,
                "No API key found. Use /login in classic mode or set OPENROUTER_API_KEY.".into(),
            );
        }

        app.add_message(MessageKind::Info, WELCOME_MSG.into());
        let location = display_repo_path(&app.repo_root);
        app.add_message(
            MessageKind::Info,
            format!("You are using Smol CLI in {location}"),
        );

        app
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        super::draw::draw(self, frame);
    }

    pub(crate) async fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        super::input::on_key(self, key).await
    }

    pub(crate) fn on_paste(&mut self, data: String) {
        super::input::on_paste(self, data);
    }

    pub(crate) fn handle_async(&mut self, event: AsyncEvent) {
        self.awaiting_response = false;
        self.caret_visible = true;
        match event {
            AsyncEvent::Error(err) => self.add_message(MessageKind::Error, err),
            AsyncEvent::ParseError {
                error,
                raw,
                prompt,
                outcome,
            } => {
                self.render_plan_and_actions(&outcome.plan, &outcome.reads, &outcome.creates);
                self.add_message(
                    MessageKind::Error,
                    format!("Model did not return valid edits: {error}"),
                );
                self.add_message(MessageKind::Info, format!("Raw response: {raw}"));
                self.last_usage = outcome.response.usage.clone();
                if let Some(tokens) = outcome.response.usage.as_ref().and_then(|u| u.total_tokens) {
                    self.total_tokens_used += tokens as u64;
                }
                let mut summary = agent::summarize_turn(&prompt, &outcome);
                summary.push_str("\nParse error.");
                self.push_memory_entry(summary);
            }
            AsyncEvent::Edits {
                prompt,
                batch,
                outcome,
            } => {
                self.render_plan_and_actions(&outcome.plan, &outcome.reads, &outcome.creates);

                // Check if this is an informational query
                let is_informational = outcome.plan.iter().any(|step| step.description.to_lowercase().contains("answer"));

                // Try to parse actions from the response
                if let Ok(actions) = edits::parse_actions(&outcome.response.content) {
                    let has_answer = actions.iter().any(|action| matches!(action, edits::Action::ProvideAnswer { .. }));

                    if has_answer {
                        // Display the answer
                        for action in actions {
                            if let edits::Action::ProvideAnswer { answer } = action {
                                self.add_message(MessageKind::Info, answer);
                            }
                        }
                        self.add_message(MessageKind::Tool, "Analysis complete.".into());
                    } else if is_informational {
                        // For informational queries without explicit answer, provide a fallback response
                        self.add_message(MessageKind::Info, "Codebase exploration completed. Here's what I found:".into());

                        // Show what was explored
                        for action in &actions {
                            match action {
                                edits::Action::ReadFile { path } => {
                                    self.add_message(MessageKind::Tool, format!("- Read file: {}", path));
                                }
                                edits::Action::ListDirectory { path } => {
                                    self.add_message(MessageKind::Tool, format!("- Listed directory: {}", path));
                                }
                                _ => {}
                            }
                        }

                        // Show plan execution results
                        for read in &outcome.reads {
                            match &read.outcome {
                                agent::ReadOutcome::Success { bytes } => {
                                    self.add_message(MessageKind::Info, format!("- Successfully read {} ({} bytes)", read.path, bytes));
                                }
                                agent::ReadOutcome::Failed { error } => {
                                    self.add_message(MessageKind::Info, format!("- Failed to read {}: {}", read.path, error));
                                }
                                agent::ReadOutcome::Skipped => {}
                            }
                        }

                        self.add_message(MessageKind::Info, "".into());
                        self.add_message(MessageKind::Info, "This appears to be a Rust CLI application with AI agent capabilities.".into());
                        self.add_message(MessageKind::Info, "For more detailed analysis, please ask specific questions about particular files or features.".into());
                    } else if batch.edits.is_empty() {
                        self.add_message(MessageKind::Info, "No edits proposed.".into());
                    } else if let Err(err) = self.begin_review(batch) {
                        self.add_message(
                            MessageKind::Error,
                            format!("Failed to prepare edits: {err}"),
                        );
                    }
                } else if batch.edits.is_empty() {
                    self.add_message(MessageKind::Info, "No edits proposed.".into());
                } else if let Err(err) = self.begin_review(batch) {
                    self.add_message(
                        MessageKind::Error,
                        format!("Failed to prepare edits: {err}"),
                    );
                }

                self.last_usage = outcome.response.usage.clone();
                if let Some(tokens) = outcome.response.usage.as_ref().and_then(|u| u.total_tokens) {
                    self.total_tokens_used += tokens as u64;
                }
                self.push_memory_entry(agent::summarize_turn(&prompt, &outcome));
            }
        }
    }

    fn render_plan_and_actions(
        &mut self,
        plan: &[agent::PlanStep],
        reads: &[agent::ReadLog],
        creates: &[agent::CreateLog],
    ) {
        if !plan.is_empty() {
            // Reset completed_steps for new plan
            self.completed_steps = vec![false; plan.len()];

            self.add_message(MessageKind::Info, "Plan:".into());
            for (idx, step) in plan.iter().enumerate() {
                let checkbox = if self.completed_steps.get(idx).copied().unwrap_or(false) {
                    "✓"
                } else {
                    "□"
                };

                let mut annotations = Vec::new();
                if let Some(path) = &step.read {
                    annotations.push(format!("read {}", path));
                }
                if let Some(path) = &step.create {
                    annotations.push(format!("create {}", path));
                }
                if annotations.is_empty() {
                    self.add_message(
                        MessageKind::Info,
                        format!("  {} {}. {}", checkbox, idx + 1, step.description),
                    );
                } else {
                    self.add_message(
                        MessageKind::Info,
                        format!(
                            "  {} {}. {} [{}]",
                            checkbox,
                            idx + 1,
                            step.description,
                            annotations.join(", ")
                        ),
                    );
                }
            }
        }

        // Update completed steps based on reads and creates
        for log in reads {
            if let agent::ReadOutcome::Success { .. } = &log.outcome {
                // Mark read steps as completed
                for (idx, step) in plan.iter().enumerate() {
                    if let Some(read_path) = &step.read {
                        if log.path == *read_path {
                            if let Some(completed) = self.completed_steps.get_mut(idx) {
                                *completed = true;
                            }
                        }
                    }
                }
            }
        }

        for log in creates {
            if let agent::CreateOutcome::Created = &log.outcome {
                // Mark create steps as completed
                for (idx, step) in plan.iter().enumerate() {
                    if let Some(create_path) = &step.create {
                        if log.path == *create_path {
                            if let Some(completed) = self.completed_steps.get_mut(idx) {
                                *completed = true;
                            }
                        }
                    }
                }
            }
        }

        // Mark list_directory steps as completed
        for (idx, step) in plan.iter().enumerate() {
            if step.description.contains("List directory") && !self.completed_steps.get(idx).copied().unwrap_or(false) {
                // Check if we've done any directory listing
                if !reads.is_empty() || !creates.is_empty() {
                    if let Some(completed) = self.completed_steps.get_mut(idx) {
                        *completed = true;
                    }
                }
            }
        }

        for log in reads {
            self.add_message(MessageKind::Info, agent::format_read_log(log));
        }

        for log in creates {
            self.add_message(MessageKind::Info, agent::format_create_log(log));
        }
    }

    fn push_memory_entry(&mut self, entry: String) {
        self.memory.push(entry);
        if self.memory.len() > 6 {
            self.memory.remove(0);
        }
    }

    pub(crate) fn toggle_caret(&mut self) {
        self.caret_visible = !self.caret_visible;
    }

    pub(crate) fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub(super) fn add_message(&mut self, kind: MessageKind, content: String) {
        let was_at_bottom = self.activity_scroll >= self.messages.len().saturating_sub(1);
        self.messages.push(Message { kind, content });
        if self.messages.len() > 200 {
            let removed = self.messages.len() - 200;
            self.messages.drain(0..removed);
            // Adjust scroll position
            self.activity_scroll = self.activity_scroll.saturating_sub(removed);
        }
        // Auto-scroll to bottom if user was already at bottom
        if was_at_bottom {
            self.activity_scroll = self.messages.len().saturating_sub(1);
        }
    }

    pub(super) fn reset_input(&mut self) {
        self.textarea = build_textarea();
        self.view_offset = (0, 0);
        self.caret_visible = true;
    }

    pub(super) async fn submit_prompt(&mut self) -> Result<()> {
        super::actions::submit_prompt(self).await
    }

    pub(super) fn begin_review(&mut self, batch: edits::EditBatch) -> Result<()> {
        let mut edits = Vec::new();
        let backup_root = timestamp_dir()?;

        for e in batch.edits {
            if is_write_blocked(&e.path) {
                self.add_message(
                    MessageKind::Warn,
                    format!("Skipping suspicious path: {}", e.path),
                );
                continue;
            }

            let abs = match fsutil::ensure_inside_repo(&self.repo_root, Path::new(&e.path)) {
                Ok(p) => p,
                Err(err) => {
                    self.add_message(
                        MessageKind::Error,
                        format!("Invalid path {}: {err}", e.path),
                    );
                    continue;
                }
            };

            let old = match fs::read_to_string(&abs) {
                Ok(s) => s,
                Err(err) => {
                    self.add_message(
                        MessageKind::Error,
                        format!("Failed to read {}: {err}", e.path),
                    );
                    continue;
                }
            };

            let new = match edits::apply_edit(&old, &e) {
                Ok(n) => n,
                Err(err) => {
                    self.add_message(MessageKind::Warn, format!("Skipping {}: {err}", e.path));
                    continue;
                }
            };

            if old == new {
                self.add_message(MessageKind::Info, format!("No change for {}", e.path));
                continue;
            }

            let diff = diff::unified_diff(&old, &new, &e.path);
            edits.push(PreparedEdit {
                path: e.path,
                abs_path: abs,
                diff,
                rationale: e.rationale,
                new_contents: new,
            });
        }

        if edits.is_empty() {
            self.add_message(MessageKind::Info, "No applicable edits.".into());
            return Ok(());
        }

        self.review = Some(ReviewState {
            edits,
            index: 0,
            backup_root,
        });
        self.caret_visible = true;
        if let Some(review) = &self.review {
            self.add_message(
                MessageKind::Info,
                format!(
                    "Proposed edits ready for review ({} items).",
                    review.edits.len()
                ),
            );
        }
        Ok(())
    }

    pub(super) fn apply_current(&mut self) -> Result<()> {
        super::review::apply_current(self)
    }

    pub(super) fn skip_current(&mut self, reason: &str) {
        super::review::skip_current(self, reason);
    }

    pub(super) fn undo_last(&mut self) {
        super::review::undo_last(self);
    }
}

pub(super) struct ModelPickerState {
    pub(super) index: usize,
}

#[derive(Clone)]
pub(super) struct Message {
    pub(super) kind: MessageKind,
    pub(super) content: String,
}

#[derive(Clone)]
pub(super) enum MessageKind {
    User,
    Info,
    Warn,
    Error,
    Tool,
}

pub(super) struct SuggestionInfo {
    pub(super) token: TokenInfo,
    pub(super) matches: Vec<String>,
}

pub(super) enum SuggestionKind {
    Command,
    File,
}

pub(super) struct TokenInfo {
    pub(super) prefix: String,
    pub(super) row: usize,
    pub(super) start_col: usize,
    pub(super) cursor_col: usize,
    pub(super) kind: SuggestionKind,
}

pub enum AsyncEvent {
    Error(String),
    ParseError {
        error: String,
        raw: String,
        prompt: String,
        outcome: agent::AgentOutcome,
    },
    Edits {
        prompt: String,
        batch: edits::EditBatch,
        outcome: agent::AgentOutcome,
    },
}

pub(super) fn build_context(memory: &[String]) -> Result<String> {
    let mut ctx = String::new();
    if let Ok(readme) = fs::read_to_string("README.md") {
        ctx.push_str("README.md:\n");
        ctx.push_str(&truncate(&readme, 10_000));
    }
    if !memory.is_empty() {
        ctx.push_str("\n\n# Conversation\n");
        for entry in memory {
            ctx.push_str(entry);
            ctx.push_str("\n---\n");
        }
    }
    Ok(ctx)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }

    let mut end = 0;
    for (idx, ch) in s.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max {
            break;
        }
        end = next;
    }

    s[..end].to_string()
}

fn timestamp_dir() -> Result<PathBuf> {
    let smol = fsutil::smol_dir()?;
    let backups = smol.join("backups");
    fs::create_dir_all(&backups).ok();
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let dir = backups.join(format!("{}", now));
    fs::create_dir_all(&dir).ok();
    Ok(dir)
}

fn is_write_blocked(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('.')
}

pub(super) fn target_from_backup(repo_root: &Path, backup: &Path) -> Option<PathBuf> {
    let smol = fsutil::smol_dir().ok()?;
    let backups = smol.join("backups");
    let rel = backup.strip_prefix(&backups).ok()?;
    let mut comps = rel.components();
    comps.next()?; // timestamp
    let stripped: PathBuf = comps.collect();
    Some(repo_root.join(stripped))
}

fn build_textarea() -> TextArea<'static> {
    let mut textarea = TextArea::default();
    textarea.set_placeholder_text("Describe the change you want");
    textarea.set_style(Style::default().fg(PROMPT_TEXT));
    textarea.set_cursor_line_style(Style::default());
    textarea
}

fn display_repo_path(path: &Path) -> String {
    if let Ok(home) = std::env::var("HOME") {
        let home_path = Path::new(&home);
        if let Ok(stripped) = path.strip_prefix(home_path) {
            let display = stripped.to_string_lossy();
            return format!("~/{}", display.trim_start_matches('/'));
        }
    }
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncate_preserves_ascii_within_limit() {
        assert_eq!(truncate("hello world", 5), "hello");
    }

    #[test]
    fn truncate_respects_utf8_boundaries() {
        let sample = "éèê"; // multibyte characters
        assert_eq!(truncate(sample, 4), "éè");
    }

    #[test]
    fn truncate_returns_empty_when_limit_too_small_for_char() {
        assert_eq!(truncate("é", 1), "");
    }
}
