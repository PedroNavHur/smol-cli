use ratatui::{
    prelude::*,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::review::ReviewState;
use super::state::{App, MessageKind};
use crate::llm;
use crate::ui::{
    app::prompt,
    theme::{
        ACTIVITY_BORDER, BANNER_BORDER, BANNER_CAT_EAR, BANNER_CAT_EYE, BANNER_CAT_MOUTH,
        BANNER_CAT_WHISKER, BANNER_TEXT, STATUS_TEXT, UI_BORDER_TYPE,
    },
};

pub(super) fn draw(app: &mut App, frame: &mut Frame) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Percentage(65),
                Constraint::Percentage(28),
                Constraint::Length(2),
            ]
            .as_ref(),
        )
        .split(frame.area());

    draw_banner(frame, layout[0]);

    let history = render_history(app);
    let (history_area, picker_area) = if app.review.is_none() {
        if let (Some(models), Some(picker)) = (&app.models, &app.model_picker) {
            let picker_height = picker_height(models);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(3),
                    Constraint::Length(picker_height.max(3)),
                ])
                .split(layout[1]);
            (chunks[0], Some((chunks[1], picker.index)))
        } else {
            (layout[1], None)
        }
    } else {
        (layout[1], None)
    };

    let history_block = Paragraph::new(history)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(UI_BORDER_TYPE)
                .border_style(Style::default().fg(ACTIVITY_BORDER))
                .title("Activity"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(history_block, history_area);

    if let Some(review) = &app.review {
        let review_block = render_review(review);
        frame.render_widget(review_block, layout[1]);
    } else if let (Some((area, selected)), Some(models)) = (picker_area, app.models.as_ref()) {
        let picker_block = render_model_picker(models, selected);
        frame.render_widget(picker_block, area);
    }

    prompt::draw_prompt(app, frame, layout[2]);
    draw_status(app, frame, layout[3]);
}

fn draw_banner(frame: &mut Frame, area: Rect) {
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

fn draw_status(app: &App, frame: &mut Frame, area: Rect) {
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
        Span::raw("Model: "),
        Span::styled(&app.cfg.provider.model, Style::default().fg(Color::Cyan)),
    ];
    let line = Line::from(spans);
    let paragraph = Paragraph::new(line)
        .alignment(Alignment::Left)
        .style(Style::default().fg(STATUS_TEXT));
    frame.render_widget(paragraph, area);
}

fn render_history(app: &App) -> Vec<Line<'static>> {
    app.messages
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

fn render_review(review: &ReviewState) -> Paragraph<'static> {
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

fn render_model_picker(models: &[llm::Model], selected: usize) -> Paragraph<'static> {
    let mut lines = Vec::new();
    lines.push(Line::raw("Select a model (↑/↓, Enter, Esc)"));
    let len = models.len();
    let window = 8usize;
    let mut start = selected.saturating_sub(window / 2);
    if start + window > len {
        start = start.saturating_sub((start + window).saturating_sub(len));
    }
    if len > window {
        start = start.min(len - window);
    } else {
        start = 0;
    }
    let end = (start + window).min(len);
    for idx in start..end {
        let model = &models[idx];
        let prefix = if idx == selected { ">" } else { " " };
        let style = if idx == selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        lines.push(Line::styled(
            format!("{prefix} {} ({})", model.name, model.id),
            style,
        ));
    }

    Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(UI_BORDER_TYPE)
                .title("Models"),
        )
        .wrap(Wrap { trim: false })
}

fn picker_height(models: &[llm::Model]) -> u16 {
    let len = models.len().min(8);
    (len as u16).saturating_add(2)
}
