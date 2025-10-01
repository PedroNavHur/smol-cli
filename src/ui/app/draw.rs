use ratatui::{
    prelude::*,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::review::ReviewState;
use super::state::{App, MessageKind};
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
                Constraint::Length(5),
                Constraint::Percentage(65),
                Constraint::Percentage(28),
                Constraint::Length(2),
            ]
            .as_ref(),
        )
        .split(frame.area());

    draw_banner(frame, layout[0]);

    let history = render_history(app);
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

    if let Some(review) = &app.review {
        let review_block = render_review(review);
        frame.render_widget(review_block, layout[1]);
    }

    prompt::draw_prompt(app, frame, layout[2]);
    draw_status(frame, layout[3]);
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

fn draw_status(frame: &mut Frame, area: Rect) {
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
