use ratatui::{
    prelude::*,
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Wrap},
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
    let prompt_lines = app.textarea.lines().len().max(1).min(10) as u16;
    let has_plan = app.current_plan.is_some();
    let has_actions = app.messages.iter().any(|m| m.kind == MessageKind::Tool);
    let constraints = match (has_plan, has_actions) {
        (true, true) => vec![
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(prompt_lines + 3),
            Constraint::Length(2),
        ],
        (true, false) => vec![
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(10),
            Constraint::Length(prompt_lines + 3),
            Constraint::Length(2),
        ],
        (false, true) => vec![
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(prompt_lines + 3),
            Constraint::Length(2),
        ],
        (false, false) => vec![
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(prompt_lines + 3),
            Constraint::Length(2),
        ],
    };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(&constraints)
        .split(frame.area());

    let mut layout_idx = 0;
    draw_banner(frame, layout[layout_idx]);
    layout_idx += 1;

    if has_plan {
        render_plan(app, frame, layout[layout_idx]);
        layout_idx += 1;
    }
    if has_actions {
        render_actions(app, frame, layout[layout_idx]);
        layout_idx += 1;
    }

    let history_layout_idx = layout_idx;
    let (history_area, picker_area) = if app.review.is_none() {
        if let (Some(models), Some(picker)) = (&app.models, &app.model_picker) {
            let picker_height = picker_height(models);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(picker_height.max(3))])
                .split(layout[history_layout_idx]);
            (chunks[0], Some((chunks[1], picker.index)))
        } else {
            (layout[history_layout_idx], None)
        }
    } else {
        (layout[history_layout_idx], None)
    };
    layout_idx += 1;

    let history = render_history(app, history_area.width as usize);

    // Always target the most recent message when auto-scroll is enabled
    let target_scroll = app.messages.len().saturating_sub(1);
    // Auto-scroll if enabled
    if app.auto_scroll_enabled {
        app.activity_scroll = target_scroll;
        app.auto_scroll_enabled = false;
    }

    let history_block = Paragraph::new(history)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(UI_BORDER_TYPE)
                .border_style(Style::default().fg(ACTIVITY_BORDER))
                .title("Activity"),
        )
        .scroll((
            (app.activity_scroll.saturating_mul(2))
                .try_into()
                .unwrap_or(0),
            0,
        ))
        .wrap(Wrap { trim: false });
    frame.render_widget(history_block, history_area);

    if let Some(review) = &app.review {
        let review_block = render_review(review);
        frame.render_widget(review_block, history_area);
    } else if let (Some((area, selected)), Some(models)) = (picker_area, app.models.as_ref()) {
        let picker_block = render_model_picker(models, selected);
        frame.render_widget(picker_block, area);
    }

    prompt::draw_prompt(app, frame, layout[layout_idx]);
    layout_idx += 1;
    draw_status(app, frame, layout[layout_idx]);
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
        let spent_cents = app
            .last_usage
            .as_ref()
            .map(|usage| estimate_cost_cents(usage, model))
            .unwrap_or(None);
        if let Some(cents) = spent_cents {
            first_line_spans.push(Span::raw("   Spent: "));
            first_line_spans.push(Span::styled(
                format!("{:.4}¢", cents),
                Style::default().fg(Color::Green),
            ));
        }
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

    let lines = vec![Line::from(first_line_spans), Line::from(second_line_spans)];
    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .style(Style::default().fg(STATUS_TEXT));
    frame.render_widget(paragraph, area);
}

fn render_plan(app: &App, frame: &mut Frame, area: Rect) {
    let mut lines = Vec::new();
    if let Some(plan) = &app.current_plan {
        lines.push(Line::from(Span::styled(
            "Plan:",
            Style::default().fg(Color::Gray),
        )));
        for (idx, step) in plan.iter().enumerate() {
            let checkbox = if app.completed_steps.get(idx).copied().unwrap_or(false) {
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
            let content = if annotations.is_empty() {
                format!("  {} {}. {}", checkbox, idx + 1, step.description)
            } else {
                format!(
                    "  {} {}. {} [{}]",
                    checkbox,
                    idx + 1,
                    step.description,
                    annotations.join(", ")
                )
            };
            lines.push(Line::from(Span::styled(
                content,
                Style::default().fg(Color::Gray),
            )));
        }
    }
    let plan_block = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(UI_BORDER_TYPE)
                .style(Style::default().bg(Color::Black))
                .title("Plan"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(plan_block, area);
}

fn render_actions(app: &App, frame: &mut Frame, area: Rect) {
    let mut lines = Vec::new();
    for message in &app.messages {
        if message.kind == MessageKind::Tool {
            let style = match message.kind {
                MessageKind::User => Style::default().fg(Color::Cyan),
                MessageKind::Warn => Style::default().fg(Color::Yellow),
                MessageKind::Error => Style::default().fg(Color::Red),
                MessageKind::Info => Style::default().fg(Color::Gray),
                MessageKind::Tool => Style::default().fg(Color::DarkGray),
            };
            let spans = parse_message(&message.content, style, area.width as usize);
            lines.push(Line::from(spans));
        }
    }
    let actions_block = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(UI_BORDER_TYPE)
                .style(Style::default().bg(Color::Rgb(50, 50, 50)))
                .title("Actions"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(actions_block, area);
}

fn render_history(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for message in &app.messages {
        if message.kind != MessageKind::Tool {
            let style = match message.kind {
                MessageKind::User => Style::default().fg(Color::Cyan),
                MessageKind::Warn => Style::default().fg(Color::Yellow),
                MessageKind::Error => Style::default().fg(Color::Red),
                MessageKind::Info => Style::default().fg(Color::Gray),
                MessageKind::Tool => Style::default().fg(Color::DarkGray),
            };
            let spans = parse_message(&message.content, style, width);
            lines.push(Line::from(spans));
            lines.push(Line::from(Span::raw(""))); // Add empty line between messages
        }
    }
    lines
}

fn parse_message(content: &str, base_style: Style, _width: usize) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut remaining = content;
    while let Some(start) = remaining.find("[highlight]") {
        if start > 0 {
            spans.push(Span::styled(remaining[..start].to_string(), base_style));
        }
        remaining = &remaining[start + 11..]; // len("[highlight]")
        if let Some(end) = remaining.find("[/highlight]") {
            spans.push(Span::styled(
                remaining[..end].to_string(),
                base_style.fg(Color::Cyan),
            ));
            remaining = &remaining[end + 12..]; // len("[/highlight]")
        } else {
            // malformed, add rest
            spans.push(Span::styled(remaining.to_string(), base_style));
            break;
        }
    }
    if !remaining.is_empty() {
        spans.push(Span::styled(remaining.to_string(), base_style));
    }
    spans
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
                .style(Style::default().bg(Color::Rgb(30, 30, 30)))
                .padding(Padding::uniform(1))
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
    cost.map(|c| format!("${:.2}/M", c * 1_000_000.0))
        .unwrap_or_else(|| "--".into())
}

fn estimate_cost_cents(usage: &llm::Usage, model: &llm::Model) -> Option<f64> {
    let prompt_tokens = usage.prompt_tokens? as f64;
    let completion_tokens = usage.completion_tokens.unwrap_or(0) as f64;
    let prompt_rate = model.prompt_cost?;
    let completion_rate = model.completion_cost?;
    let total_cost =
        (prompt_tokens * prompt_rate + completion_tokens * completion_rate) / 1_000_000.0;
    Some(total_cost * 100.0)
}
