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
                .constraints([Constraint::Min(3), Constraint::Length(picker_height.max(3))])
                .split(layout[1]);
            (chunks[0], Some((chunks[1], picker.index)))
        } else {
            (layout[1], None)
        }
    } else {
        (layout[1], None)
    };

    // Calculate how many messages can fit
    let messages_per_screen = (history_area.height / 2).max(1) as usize;
    // Auto-scroll to show the latest messages
    let target_scroll = app.messages.len().saturating_sub(messages_per_screen);
    // Only auto-scroll if not manually scrolled up
    if app.activity_scroll == 0 || app.activity_scroll < target_scroll {
        app.activity_scroll = target_scroll;
    }

    let history_block = Paragraph::new(history)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(UI_BORDER_TYPE)
                .border_style(Style::default().fg(ACTIVITY_BORDER))
                .title("Activity"),
        )
        .scroll(((app.activity_scroll * 2).try_into().unwrap(), 0))
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

    let mut first_line_spans = vec![
        Span::raw("Model: "),
        Span::styled(&app.cfg.provider.model, Style::default().fg(Color::Cyan)),
    ];

    if let Some(model) = &app.current_model {
        first_line_spans.push(Span::raw("   Rate in "));
        first_line_spans.push(Span::styled(
            format_cost(model.prompt_cost),
            Style::default().fg(Color::Yellow),
        ));
        first_line_spans.push(Span::raw(" out "));
        first_line_spans.push(Span::styled(
            format_cost(model.completion_cost),
            Style::default().fg(Color::Yellow),
        ));
        first_line_spans.push(Span::raw(" ctx "));
        first_line_spans.push(Span::styled(
            format_ctx_value_opt(model.context_length),
            Style::default().fg(Color::Yellow),
        ));
    }

    let icon_style = Style::default().fg(STATUS_TEXT);
    let mut second_line_spans = vec![
        Span::styled("⏎", icon_style),
        Span::raw(" send   "),
        Span::styled("⇧/⌥/⌃⏎", icon_style),
        Span::raw(" newline   "),
        Span::styled("⌃U/D", icon_style),
        Span::raw(" scroll   "),
        Span::styled("⌃C", icon_style),
        Span::raw(" quit   "),
    ];

    if let Some(usage) = &app.last_usage {
        second_line_spans.push(Span::raw("   Tokens: "));
        if let Some(total) = usage.total_tokens {
            second_line_spans.push(Span::styled(
                total.to_string(),
                Style::default().fg(Color::Yellow),
            ));
        } else {
            second_line_spans.push(Span::raw("--"));
        }

        if usage.prompt_tokens.is_some() || usage.completion_tokens.is_some() {
            second_line_spans.push(Span::raw(" (prompt "));
            let prompt = usage
                .prompt_tokens
                .map(|p| p.to_string())
                .unwrap_or_else(|| "--".into());
            second_line_spans.push(Span::styled(prompt, Style::default().fg(Color::Yellow)));
            second_line_spans.push(Span::raw(", completion "));
            let completion = usage
                .completion_tokens
                .map(|c| c.to_string())
                .unwrap_or_else(|| "--".into());
            second_line_spans.push(Span::styled(completion, Style::default().fg(Color::Yellow)));
            second_line_spans.push(Span::raw(")"));
        }

        if let Some(cost) = usage.total_cost {
            second_line_spans.push(Span::raw("   Cost: $"));
            second_line_spans.push(Span::styled(
                format!("{cost:.4}"),
                Style::default().fg(Color::Green),
            ));
        }
    }

    second_line_spans.push(Span::styled(
        format!("   {} tokens used", app.total_tokens_used),
        Style::default().fg(Color::Yellow),
    ));

    // Add scroll indicator
    if app.messages.len() > 0 {
        let total = app.messages.len();
        let current = app.activity_scroll + 1;
        second_line_spans.push(Span::raw(format!("   {}/{}", current.min(total), total)));
    }

    let lines = vec![
        Line::from(first_line_spans),
        Line::from(second_line_spans),
    ];
    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .style(Style::default().fg(STATUS_TEXT));
    frame.render_widget(paragraph, area);
}

fn render_history(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for message in &app.messages {
        let style = match message.kind {
            MessageKind::User => Style::default().fg(Color::Cyan),
            MessageKind::Warn => Style::default().fg(Color::Yellow),
            MessageKind::Error => Style::default().fg(Color::Red),
            MessageKind::Info => Style::default().fg(Color::Gray),
            MessageKind::Tool => Style::default().fg(Color::DarkGray),
        };
        lines.push(Line::from(Span::styled(message.content.clone(), style)));
        lines.push(Line::from(Span::raw(""))); // Add empty line between messages
    }
    lines
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
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let ctx = format_ctx(model.context_length.as_ref());
        let prompt_cost = model
            .prompt_cost
            .map(|c| format!("${:.2}/M", c * 1_000_000.0))
            .unwrap_or_else(|| "--".into());
        let completion_cost = model
            .completion_cost
            .map(|c| format!("${:.2}/M", c * 1_000_000.0))
            .unwrap_or_else(|| "--".into());
        lines.push(Line::styled(
            format!(
                "{prefix} {} ({})  in {}  out {}  {}",
                model.name, model.id, prompt_cost, completion_cost, ctx
            ),
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

fn format_ctx(ctx: Option<&u32>) -> String {
    match ctx {
        Some(c) => format!("ctx {}", format_ctx_value(*c)),
        None => "ctx --".into(),
    }
}

fn format_ctx_value(c: u32) -> String {
    if c % 1000 == 0 {
        format!("{}K", c / 1000)
    } else {
        format!("{:.1}K", c as f32 / 1000.0)
    }
}

fn format_ctx_value_opt(ctx: Option<u32>) -> String {
    ctx.map(format_ctx_value).unwrap_or_else(|| "--".into())
}

fn format_cost(cost: Option<f64>) -> String {
    cost.map(|c| format!("${:.2}/M", c * 1_000.0))
        .unwrap_or_else(|| "--".into())
}
