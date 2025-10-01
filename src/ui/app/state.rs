use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    prelude::*,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::{spawn, sync::mpsc::UnboundedSender};
use tui_textarea::{Input, TextArea};

use crate::{config, diff, edits, fsutil, llm};

use super::prompt;
use crate::ui::theme::{
    ACTIVITY_BORDER, BANNER_BORDER, BANNER_CAT_EAR, BANNER_CAT_EYE, BANNER_CAT_MOUTH,
    BANNER_CAT_WHISKER, BANNER_TEXT, PROMPT_TEXT, STATUS_TEXT, UI_BORDER_TYPE,
};

const WELCOME_MSG: &str =
    "Smol CLI — TUI chat. Enter prompts below. y/apply, n/skip during review.";

pub(super) const COMMANDS: &[&str] = &[
    "/help", "/login", "/model", "/clear", "/stats", "/undo", "/quit", "/exit",
];

pub struct App {
    pub(super) cfg: config::AppConfig,
    pub(super) repo_root: PathBuf,
    pub(super) tx: UnboundedSender<AsyncEvent>,
    pub(super) textarea: TextArea<'static>,
    messages: Vec<Message>,
    pub(super) view_offset: (u16, u16),
    history: Vec<String>,
    pub(super) awaiting_response: bool,
    pub(super) review: Option<ReviewState>,
    last_backups: Vec<PathBuf>,
    should_quit: bool,
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
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Length(5),
                    Constraint::Percentage(65),
                    Constraint::Percentage(28),
                    Constraint::Length(2),
                ]
                .as_ref(),
            )
            .split(frame.area());

        self.draw_banner(frame, layout[0]);

        let history = self.render_history();
        let history_block = Paragraph::new(history)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(UI_BORDER_TYPE)
                    .border_style(Style::default().fg(ACTIVITY_BORDER))
                    .title("Activity"),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(history_block, layout[1]);

        if let Some(review) = &self.review {
            let review_block = self.render_review(review);
            frame.render_widget(review_block, layout[1]);
        }

        prompt::draw_prompt(self, frame, layout[2]);
        self.draw_status(frame, layout[3]);
    }

    pub(crate) async fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        self.caret_visible = true;
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return Ok(());
        }

        if key.code == KeyCode::Tab && key.modifiers.is_empty() {
            if prompt::try_accept_suggestion(self) {
                return Ok(());
            }
        }

        if self.review.is_some() {
            match key.code {
                KeyCode::Char('y') => {
                    if let Err(err) = self.apply_current() {
                        self.add_message(MessageKind::Error, format!("Apply failed: {err}"));
                    }
                }
                KeyCode::Char('n') => {
                    self.skip_current("Skipped by user");
                }
                KeyCode::Char('b') => {
                    self.review = None;
                    self.add_message(MessageKind::Info, "Exited review.".into());
                    self.caret_visible = true;
                }
                _ => {}
            }
            return Ok(());
        }

        if key.code == KeyCode::Char('z') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.undo_last();
            return Ok(());
        }

        if key.code == KeyCode::Enter && key.modifiers.is_empty() {
            self.submit_prompt().await?;
            return Ok(());
        }

        let input = Input::from(Event::Key(key));
        self.textarea.input(input);
        Ok(())
    }

    pub(crate) fn on_paste(&mut self, data: String) {
        if self.review.is_none() {
            self.textarea.insert_str(&data);
            self.caret_visible = true;
        }
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

    fn add_message(&mut self, kind: MessageKind, content: String) {
        self.messages.push(Message { kind, content });
        if self.messages.len() > 200 {
            self.messages.drain(0..self.messages.len() - 200);
        }
    }

    fn handle_command(&mut self, input: &str) -> Result<()> {
        self.caret_visible = true;
        match input {
            "/help" => self.add_message(
                MessageKind::Info,
                "/login  /model  /clear  /undo  /stats  /quit".into(),
            ),
            "/quit" | "/exit" => {
                self.should_quit = true;
            }
            "/clear" => {
                self.messages.clear();
                self.history.clear();
                self.add_message(MessageKind::Info, "History cleared.".into());
                self.add_message(MessageKind::Info, WELCOME_MSG.into());
            }
            "/stats" => {
                self.add_message(
                    MessageKind::Info,
                    format!("Messages: {}", self.history.len()),
                );
            }
            "/undo" => self.undo_last(),
            "/login" => self.add_message(
                MessageKind::Warn,
                "Temporarily unsupported here. Run `/login` in classic chat mode.".into(),
            ),
            cmd if cmd.starts_with("/model") => {
                let parts: Vec<_> = cmd.split_whitespace().collect();
                if parts.len() == 2 {
                    self.cfg.provider.model = parts[1].to_string();
                    config::save(&self.cfg)?;
                    self.add_message(
                        MessageKind::Info,
                        format!("Model set to {}", self.cfg.provider.model),
                    );
                } else {
                    self.add_message(
                        MessageKind::Warn,
                        "Usage: /model <provider/model>, e.g., openai/gpt-4o-mini".into(),
                    );
                }
            }
            _ => self.add_message(MessageKind::Warn, "Unknown command. /help".into()),
        }
        Ok(())
    }

    fn draw_banner(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(UI_BORDER_TYPE)
            .border_style(Style::default().fg(BANNER_BORDER))
            .title("Smol CLI");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let cat_line = Line::from(vec![
            Span::styled("  (", Style::default().fg(BANNER_TEXT)),
            Span::styled("=", Style::default().fg(BANNER_CAT_WHISKER)),
            Span::styled("^", Style::default().fg(BANNER_CAT_EAR)),
            Span::styled("･", Style::default().fg(BANNER_CAT_EYE)),
            Span::styled("ω", Style::default().fg(BANNER_CAT_MOUTH)),
            Span::styled("･", Style::default().fg(BANNER_CAT_EYE)),
            Span::styled("^", Style::default().fg(BANNER_CAT_EAR)),
            Span::styled("=", Style::default().fg(BANNER_CAT_WHISKER)),
            Span::styled(")", Style::default().fg(BANNER_TEXT)),
            Span::raw("  "),
            Span::styled(
                "Smol - a minimal coding agent",
                Style::default().fg(BANNER_TEXT),
            ),
        ]);
        let lines = vec![cat_line];
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let icon_style = Style::default().fg(STATUS_TEXT);
        let spans = vec![
            Span::styled("⏎", icon_style),
            Span::raw(" send   "),
            Span::styled("⇧⏎", icon_style),
            Span::raw(" newline   "),
            Span::styled("⌃C", icon_style),
            Span::raw(" quit   "),
            Span::raw("538K tokens used   "),
            Span::raw("66% context left"),
        ];
        let line = Line::from(spans);
        let paragraph = Paragraph::new(line)
            .alignment(Alignment::Left)
            .style(Style::default().fg(STATUS_TEXT));
        frame.render_widget(paragraph, area);
    }

    fn render_history(&self) -> Vec<Line<'static>> {
        self.messages
            .iter()
            .map(|m| {
                let style = match m.kind {
                    MessageKind::User => Style::default().fg(Color::Cyan),
                    MessageKind::Warn => Style::default().fg(Color::Yellow),
                    MessageKind::Error => Style::default().fg(Color::Red),
                    MessageKind::Info => Style::default().fg(Color::Gray),
                };
                Line::from(Span::styled(m.content.clone(), style))
            })
            .collect()
    }

    fn render_review(&self, review: &ReviewState) -> Paragraph<'static> {
        let mut lines = Vec::new();
        if let Some(current) = review.current_edit() {
            lines.push(Line::raw(format!(
                "Reviewing {} ({} of {})",
                current.path,
                review.index + 1,
                review.edits.len()
            )));
            if let Some(r) = &current.rationale {
                lines.push(Line::raw(format!("Reason: {r}")));
            }
            lines.push(Line::raw("Press y=apply, n=skip, b=cancel review"));
            lines.push(Line::raw("────────────────────────────────"));
            lines.extend(current.diff.lines().map(|l| Line::raw(l.to_string())));
        }
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(UI_BORDER_TYPE)
                    .title("Proposed edit"),
            )
            .wrap(Wrap { trim: false })
    }

    fn reset_input(&mut self) {
        self.textarea = build_textarea();
        self.view_offset = (0, 0);
        self.caret_visible = true;
    }

    async fn submit_prompt(&mut self) -> Result<()> {
        if self.awaiting_response {
            self.add_message(
                MessageKind::Warn,
                "Still waiting for the last response...".into(),
            );
            return Ok(());
        }

        let prompt = self.textarea.lines().join("\n");
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return Ok(());
        }

        if trimmed.starts_with('/') {
            self.reset_input();
            self.handle_command(trimmed)?;
            return Ok(());
        }

        if self.cfg.auth.api_key.is_empty() {
            self.add_message(
                MessageKind::Error,
                "Missing OpenRouter API key. Set OPENROUTER_API_KEY or use plain mode /login."
                    .into(),
            );
            self.reset_input();
            return Ok(());
        }

        self.add_message(MessageKind::User, trimmed.to_string());
        self.history.push(trimmed.to_string());
        self.reset_input();
        self.awaiting_response = true;
        self.caret_visible = true;

        let cfg = self.cfg.clone();
        let tx = self.tx.clone();
        let prompt = trimmed.to_string();

        spawn(async move {
            let event = async_handle_prompt(cfg, prompt).await;
            let _ = tx.send(event);
        });

        Ok(())
    }

    fn begin_review(&mut self, batch: edits::EditBatch) -> Result<()> {
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

    fn apply_current(&mut self) -> Result<()> {
        let (edit, backup_root) = match self
            .review
            .as_ref()
            .and_then(|r| r.current_edit().map(|e| (e.clone(), r.backup_root.clone())))
        {
            Some(tuple) => tuple,
            None => return Ok(()),
        };

        let backup_file = fsutil::backup_path(&backup_root, &edit.abs_path, &self.repo_root)?;
        fsutil::backup_and_write(&edit.abs_path, &edit.new_contents, &backup_file)?;
        self.add_message(
            MessageKind::Info,
            format!("Applied {} (backup: {})", edit.path, backup_file.display()),
        );
        self.last_backups.push(backup_file);
        self.advance_review();
        Ok(())
    }

    fn skip_current(&mut self, reason: &str) {
        if let Some(review) = &self.review {
            if let Some(current) = review.current_edit() {
                self.add_message(MessageKind::Info, format!("{}: {}", reason, current.path));
            }
        }
        self.advance_review();
    }

    fn advance_review(&mut self) {
        if let Some(review) = &mut self.review {
            review.index += 1;
            if review.index >= review.edits.len() {
                self.review = None;
                self.add_message(MessageKind::Info, "Review complete.".into());
                self.caret_visible = true;
            }
        }
    }

    fn undo_last(&mut self) {
        if let Some(backup) = self.last_backups.pop() {
            match target_from_backup(&self.repo_root, &backup) {
                Some(target) => match fs::copy(&backup, &target) {
                    Ok(_) => self
                        .add_message(MessageKind::Info, format!("Reverted {}", target.display())),
                    Err(err) => self.add_message(MessageKind::Error, format!("Undo failed: {err}")),
                },
                None => self.add_message(
                    MessageKind::Warn,
                    format!("Could not determine target for {}", backup.display()),
                ),
            }
        } else {
            self.add_message(MessageKind::Info, "Nothing to undo.".into());
        }
        self.caret_visible = true;
    }
}

#[derive(Clone)]
struct PreparedEdit {
    path: String,
    abs_path: PathBuf,
    diff: String,
    rationale: Option<String>,
    new_contents: String,
}

pub(super) struct ReviewState {
    edits: Vec<PreparedEdit>,
    index: usize,
    backup_root: PathBuf,
}

impl ReviewState {
    fn current_edit(&self) -> Option<&PreparedEdit> {
        self.edits.get(self.index)
    }
}

#[derive(Clone)]
struct Message {
    kind: MessageKind,
    content: String,
}

#[derive(Clone)]
enum MessageKind {
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

async fn async_handle_prompt(cfg: config::AppConfig, prompt: String) -> AsyncEvent {
    let context = build_context().unwrap_or_else(|_| String::new());
    match llm::propose_edits(&cfg, &prompt, &context).await {
        Ok(raw) => match edits::parse_edits(&raw) {
            Ok(batch) => AsyncEvent::Edits { batch },
            Err(err) => AsyncEvent::ParseError {
                error: err.to_string(),
                raw,
            },
        },
        Err(err) => AsyncEvent::Error(err.to_string()),
    }
}

fn build_context() -> Result<String> {
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

fn target_from_backup(repo_root: &Path, backup: &Path) -> Option<PathBuf> {
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
