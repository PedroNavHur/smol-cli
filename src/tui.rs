use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame,
    prelude::*,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tui_textarea::{Input, TextArea};

use crate::{config, diff, edits, fsutil, llm};

const WELCOME_MSG: &str =
    "Smol CLI — TUI chat. Enter prompts below. y/apply, n/skip during review.";

pub async fn run(model_override: Option<String>) -> Result<()> {
    let mut cfg = config::load()?;
    if let Some(model) = model_override {
        cfg.provider.model = model;
    }

    let repo_root = std::env::current_dir()?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let (tx, rx) = unbounded_channel();
    let mut app = App::new(cfg, repo_root, tx.clone());

    let res = run_app(&mut terminal, &mut app, rx).await;

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    res
}

async fn run_app(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    app: &mut App,
    mut rx: UnboundedReceiver<AsyncEvent>,
) -> Result<()> {
    loop {
        while let Ok(event) = rx.try_recv() {
            app.handle_async(event);
        }

        terminal.draw(|frame| app.draw(frame))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => app.on_key(key).await?,
                Event::Paste(data) => app.on_paste(data),
                Event::Resize(_, _) => {}
                Event::FocusGained | Event::FocusLost | Event::Mouse(_) => {}
            }
        }
    }

    Ok(())
}

struct App {
    cfg: config::AppConfig,
    repo_root: PathBuf,
    tx: UnboundedSender<AsyncEvent>,
    textarea: TextArea<'static>,
    messages: Vec<Message>,
    view_offset: (u16, u16),
    history: Vec<String>,
    awaiting_response: bool,
    review: Option<ReviewState>,
    last_backups: Vec<PathBuf>,
    should_quit: bool,
}

impl App {
    fn new(cfg: config::AppConfig, repo_root: PathBuf, tx: UnboundedSender<AsyncEvent>) -> Self {
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
        };

        if app.cfg.auth.api_key.is_empty() {
            app.add_message(
                MessageKind::Warn,
                "No API key found. Use /login in classic mode or set OPENROUTER_API_KEY.".into(),
            );
        }

        app.add_message(MessageKind::Info, WELCOME_MSG.into());

        app
    }

    fn add_message(&mut self, kind: MessageKind, content: String) {
        self.messages.push(Message { kind, content });
        if self.messages.len() > 200 {
            self.messages.drain(0..self.messages.len() - 200);
        }
    }

    fn handle_command(&mut self, input: &str) -> Result<()> {
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

    async fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return Ok(());
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

    fn draw(&mut self, frame: &mut Frame) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)].as_ref())
            .split(frame.area());

        let history = self.render_history();
        let history_block = Paragraph::new(history)
            .block(Block::default().borders(Borders::ALL).title("Activity"))
            .wrap(Wrap { trim: false });
        frame.render_widget(history_block, layout[0]);

        if let Some(review) = &self.review {
            let review_block = self.render_review(review);
            frame.render_widget(review_block, layout[0]);
        }

        self.draw_prompt(frame, layout[1]);
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
                    .title("Proposed edit"),
            )
            .wrap(Wrap { trim: false })
    }

    fn draw_prompt(&mut self, frame: &mut Frame, area: Rect) {
        let prompt_block = Block::default()
            .borders(Borders::ALL)
            .title("Prompt (Enter to submit, Ctrl+C to exit)");
        frame.render_widget(prompt_block.clone(), area);
        let inner = prompt_block.inner(area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let sections = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(inner);

        let caret_text = if self.awaiting_response {
            "… "
        } else if self.review.is_some() {
            "= "
        } else {
            "> "
        };
        frame.render_widget(Paragraph::new(caret_text), sections[0]);
        frame.render_widget(self.textarea.widget(), sections[1]);

        if self.review.is_none() {
            let cursor = self.textarea.cursor();
            let height = sections[1].height;
            let width = sections[1].width;
            if height == 0 || width == 0 {
                return;
            }

            let (prev_row, prev_col) = self.view_offset;
            let adjust = |prev: u16, cursor: usize, len: u16| -> u16 {
                if len == 0 {
                    return prev;
                }
                let cursor = cursor as u16;
                if cursor < prev {
                    cursor
                } else if prev + len <= cursor {
                    cursor + 1 - len
                } else {
                    prev
                }
            };

            let top_row = adjust(prev_row, cursor.0, height);
            let top_col = adjust(prev_col, cursor.1, width);
            self.view_offset = (top_row, top_col);

            let visible_row = cursor.0.saturating_sub(top_row as usize) as u16;
            let visible_col = cursor.1.saturating_sub(top_col as usize) as u16;
            let x = sections[1].x + visible_col.min(sections[1].width.saturating_sub(1));
            let y = sections[1].y + visible_row.min(sections[1].height.saturating_sub(1));
            frame.set_cursor_position(Position::new(x, y));
        }
    }

    fn on_paste(&mut self, data: String) {
        if self.review.is_none() {
            self.textarea.insert_str(&data);
        }
    }

    fn reset_input(&mut self) {
        self.textarea = build_textarea();
        self.view_offset = (0, 0);
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

        let cfg = self.cfg.clone();
        let tx = self.tx.clone();
        let prompt = trimmed.to_string();

        tokio::spawn(async move {
            let event = async_handle_prompt(cfg, prompt).await;
            let _ = tx.send(event);
        });

        Ok(())
    }

    fn handle_async(&mut self, event: AsyncEvent) {
        self.awaiting_response = false;
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

struct ReviewState {
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

enum AsyncEvent {
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
        s.to_string()
    } else {
        s[..max].to_string()
    }
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
    textarea.set_style(Style::default().fg(Color::Cyan));
    textarea
}
