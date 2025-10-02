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
use crate::ui::theme::PROMPT_TEXT;
use crate::{config, diff, edits, fsutil};

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
    pub(super) history: Vec<String>,
    pub(super) awaiting_response: bool,
    pub(super) review: Option<ReviewState>,
    pub(super) last_backups: Vec<PathBuf>,
    pub(super) should_quit: bool,
    pub(super) caret_visible: bool,
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
            history: Vec::new(),
            awaiting_response: false,
            review: None,
            last_backups: Vec::new(),
            should_quit: false,
            caret_visible: true,
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
            AsyncEvent::ParseError { error, raw } => {
                self.add_message(
                    MessageKind::Error,
                    format!("Model did not return valid edits: {error}"),
                );
                self.add_message(MessageKind::Info, format!("Raw response: {raw}"));
            }
            AsyncEvent::Edits { batch } => {
                if batch.edits.is_empty() {
                    self.add_message(MessageKind::Info, "No edits proposed.".into());
                } else if let Err(err) = self.begin_review(batch) {
                    self.add_message(
                        MessageKind::Error,
                        format!("Failed to prepare edits: {err}"),
                    );
                }
            }
        }
    }

    pub(crate) fn toggle_caret(&mut self) {
        self.caret_visible = !self.caret_visible;
    }

    pub(crate) fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub(super) fn add_message(&mut self, kind: MessageKind, content: String) {
        self.messages.push(Message { kind, content });
        if self.messages.len() > 200 {
            self.messages.drain(0..self.messages.len() - 200);
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
    ParseError { error: String, raw: String },
    Edits { batch: edits::EditBatch },
}

pub(super) fn build_context() -> Result<String> {
    let mut ctx = String::new();
    if let Ok(readme) = fs::read_to_string("README.md") {
        ctx.push_str("README.md:\n");
        ctx.push_str(&truncate(&readme, 10_000));
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
