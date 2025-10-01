use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::config;

use super::app::{App, AsyncEvent};

pub async fn run(model_override: Option<String>) -> Result<()> {
    let mut cfg = config::load()?;
    if let Some(model) = model_override {
        cfg.provider.model = model;
    }

    let repo_root: PathBuf = std::env::current_dir()?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let (tx, rx) = unbounded_channel();
    let mut app = App::new(cfg, repo_root, tx);

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
    const BLINK_INTERVAL: Duration = Duration::from_millis(500);
    let mut last_blink = Instant::now();

    loop {
        while let Ok(event) = rx.try_recv() {
            app.handle_async(event);
            last_blink = Instant::now();
        }

        if last_blink.elapsed() >= BLINK_INTERVAL {
            app.toggle_caret();
            last_blink = Instant::now();
        }

        terminal.draw(|frame| app.draw(frame))?;

        if app.should_quit() {
            break;
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    app.on_key(key).await?;
                    last_blink = Instant::now();
                }
                Event::Paste(data) => {
                    app.on_paste(data);
                    last_blink = Instant::now();
                }
                Event::Resize(_, _) => {}
                Event::FocusGained | Event::FocusLost | Event::Mouse(_) => {}
            }
        }
    }

    Ok(())
}
