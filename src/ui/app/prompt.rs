use super::state::{App, COMMANDS, SuggestionInfo, SuggestionKind, TokenInfo};
use crate::ui::theme::{PROMPT_BORDER, PROMPT_TEXT, UI_BORDER_TYPE};
use ratatui::{
    Frame,
    buffer::Buffer,
    prelude::*,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tui_textarea::{CursorMove, TextArea};
use walkdir::WalkDir;

pub(super) fn draw_prompt(app: &mut App, frame: &mut Frame, area: Rect) {
    let prompt_block = Block::default()
        .borders(Borders::ALL)
        .border_type(UI_BORDER_TYPE)
        .border_style(Style::default().fg(PROMPT_BORDER))
        .title("Prompt (Enter to submit, Ctrl+C to exit)");
    frame.render_widget(prompt_block.clone(), area);
    let inner = prompt_block.inner(area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let suggestion = gather_suggestions(app);
    let show_suggestions = suggestion
        .as_ref()
        .map(|s| !s.matches.is_empty())
        .unwrap_or(false)
        && inner.height >= 2;

    let (input_area, suggestion_area) = if show_suggestions {
        let splits = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)].as_ref())
            .split(inner);
        (splits[0], Some(splits[1]))
    } else {
        (inner, None)
    };

    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(input_area);

    let caret_char = if app.awaiting_response {
        '.'
    } else if app.review.is_some() {
        '='
    } else {
        '>'
    };
    frame.render_widget(
        Paragraph::new(format!("{caret_char} ")).style(Style::default().fg(PROMPT_BORDER)),
        sections[0],
    );
    frame.render_widget(app.textarea.widget(), sections[1]);

    if let (Some(area), Some(info)) = (suggestion_area, suggestion.as_ref()) {
        let label = match info.token.kind {
            SuggestionKind::Command => "Commands",
            SuggestionKind::File => "Files",
        };
        let text = format!("{}: {}", label, info.matches.join("   "));
        if !text.is_empty() {
            let suggestion_para = Paragraph::new(text)
                .style(Style::default().fg(PROMPT_TEXT))
                .wrap(Wrap { trim: true });
            frame.render_widget(suggestion_para, area);
        }
    }

    if app.review.is_none() {
        let cursor = app.textarea.cursor();
        let height = sections[1].height;
        let width = sections[1].width;
        if height != 0 && width != 0 {
            let (prev_row, prev_col) = app.view_offset;
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
            app.view_offset = (top_row, top_col);

            let visible_row = cursor.0.saturating_sub(top_row as usize) as u16;
            let visible_col = cursor.1.saturating_sub(top_col as usize) as u16;

            let active_token = current_token_any(app);
            highlight_tokens(
                frame.buffer_mut(),
                sections[1],
                app.textarea.lines(),
                top_row as usize,
                top_col as usize,
                active_token.as_ref(),
            );

            if let Some(info) = suggestion.as_ref() {
                if let Some(rem) = info
                    .matches
                    .first()
                    .and_then(|s| s.strip_prefix(&info.token.prefix))
                {
                    if !rem.is_empty() {
                        let x =
                            sections[1].x + visible_col.min(sections[1].width.saturating_sub(1));
                        let y =
                            sections[1].y + visible_row.min(sections[1].height.saturating_sub(1));
                        let max_width =
                            sections[1].width.saturating_sub(visible_col).max(0) as usize;
                        if max_width > 0 {
                            frame.buffer_mut().set_stringn(
                                x,
                                y,
                                rem,
                                max_width,
                                Style::default().fg(PROMPT_TEXT).add_modifier(Modifier::DIM),
                            );
                        }
                    }
                }
            }

            if app.caret_visible {
                let x = sections[1].x + visible_col.min(sections[1].width.saturating_sub(1));
                let y = sections[1].y + visible_row.min(sections[1].height.saturating_sub(1));
                frame.set_cursor_position(Position::new(x, y));
            }
        }
    }
}

pub(super) fn try_accept_suggestion(app: &mut App) -> bool {
    let info = match gather_suggestions(app) {
        Some(info) if !info.matches.is_empty() => info,
        _ => return false,
    };

    let replacement = match info.token.kind {
        SuggestionKind::Command => {
            let mut text = info.matches[0].clone();
            if !text.ends_with(' ') {
                text.push(' ');
            }
            text
        }
        SuggestionKind::File => info.matches[0].clone(),
    };

    let mut lines = app.textarea.lines().to_vec();
    let line = match lines.get_mut(info.token.row) {
        Some(line) => line,
        None => return false,
    };

    let start_byte = col_to_byte(line, info.token.start_col);
    let end_byte = col_to_byte(line, info.token.cursor_col);
    let mut new_line = String::new();
    new_line.push_str(&line[..start_byte]);
    new_line.push_str(&replacement);
    new_line.push_str(&line[end_byte..]);
    *line = new_line;

    let new_cursor_col = info.token.start_col + replacement.chars().count();
    set_textarea_with_cursor(app, lines, info.token.row, new_cursor_col);
    app.caret_visible = true;
    true
}

fn set_textarea_with_cursor(app: &mut App, lines: Vec<String>, row: usize, col: usize) {
    let mut textarea = TextArea::from(lines);
    textarea.set_placeholder_text("Describe the change you want");
    textarea.set_style(Style::default().fg(PROMPT_TEXT));
    textarea.move_cursor(CursorMove::Jump(row as u16, col as u16));
    app.textarea = textarea;
    app.view_offset = (0, 0);
}

fn gather_suggestions(app: &App) -> Option<SuggestionInfo> {
    if let Some(token) = current_command_token(app) {
        let matches = command_matches(&token.prefix);
        if !matches.is_empty() {
            return Some(SuggestionInfo { token, matches });
        }
    }

    if let Some(token) = current_file_token(app) {
        let matches = file_matches(app, &token.prefix);
        if !matches.is_empty() {
            return Some(SuggestionInfo { token, matches });
        }
    }

    None
}

fn current_command_token(app: &App) -> Option<TokenInfo> {
    token_at_cursor(app, '/', true, SuggestionKind::Command)
}

fn current_file_token(app: &App) -> Option<TokenInfo> {
    token_at_cursor(app, '@', true, SuggestionKind::File)
}

fn token_at_cursor(
    app: &App,
    marker: char,
    require_boundary: bool,
    kind: SuggestionKind,
) -> Option<TokenInfo> {
    let (row, col) = app.textarea.cursor();
    let lines = app.textarea.lines();
    let line = lines.get(row)?;
    let cursor_byte = col_to_byte(line, col);
    let upto_cursor = &line[..cursor_byte];
    let start_byte = upto_cursor.rfind(marker)?;

    if require_boundary && start_byte > 0 {
        if let Some(prev) = line[..start_byte].chars().last() {
            if !is_token_boundary(prev) {
                return None;
            }
        }
    }

    let prefix = &line[start_byte..cursor_byte];
    if prefix.len() > 1 && prefix[1..].chars().any(char::is_whitespace) {
        return None;
    }

    Some(TokenInfo {
        prefix: prefix.to_string(),
        row,
        start_col: line[..start_byte].chars().count(),
        cursor_col: col,
        kind,
    })
}

fn command_matches(prefix: &str) -> Vec<String> {
    COMMANDS
        .iter()
        .filter(|cmd| cmd.starts_with(prefix))
        .map(|s| (*s).to_string())
        .collect()
}

fn file_matches(app: &App, prefix: &str) -> Vec<String> {
    let search = prefix.trim_start_matches('@');
    let mut results = Vec::new();

    for entry in WalkDir::new(&app.repo_root)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        let rel = match path.strip_prefix(&app.repo_root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.starts_with(search) {
            if entry.file_type().is_dir() {
                results.push(format!("@{}/", rel_str));
            } else {
                results.push(format!("@{}", rel_str));
            }
        }
        if results.len() >= 12 {
            break;
        }
    }

    results.sort();
    results
}

fn col_to_byte(line: &str, col: usize) -> usize {
    let mut count = 0;
    for (idx, _) in line.char_indices() {
        if count == col {
            return idx;
        }
        count += 1;
    }
    line.len()
}

fn is_token_boundary(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '(' | '[' | '{' | '<' | '>' | ',' | ';' | ':' | '-' | '/' | '"' | '\'' | '='
        )
}

fn highlight_tokens(
    buffer: &mut Buffer,
    area: Rect,
    lines: &[String],
    top_row: usize,
    top_col: usize,
    active: Option<&TokenInfo>,
) {
    let height = area.height as usize;
    let width = area.width as usize;

    for row_offset in 0..height {
        let line_idx = top_row + row_offset;
        if line_idx >= lines.len() {
            break;
        }
        let line = &lines[line_idx];
        for (start_col, end_col) in collect_tokens(line) {
            if end_col <= top_col {
                continue;
            }

            let token_end = if let Some(active_token) = active {
                if active_token.row == line_idx && active_token.start_col == start_col {
                    active_token.start_col + active_token.prefix.chars().count()
                } else {
                    end_col
                }
            } else {
                end_col
            };

            let start = start_col.max(top_col);
            let end = token_end.min(top_col + width);
            if start >= end {
                continue;
            }
            let y = area.y + row_offset as u16;
            for col in start..end {
                let x = area.x + (col - top_col) as u16;
                if x >= area.x + area.width {
                    break;
                }
                if let Some(cell) = buffer.cell_mut(Position::new(x, y)) {
                    let style = cell.style().add_modifier(Modifier::UNDERLINED);
                    cell.set_style(style);
                }
            }
        }
    }
}

fn collect_tokens(line: &str) -> Vec<(usize, usize)> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        let ch = chars[i];
        if (ch == '/' || ch == '@') && (i == 0 || is_token_boundary(chars[i.saturating_sub(1)])) {
            let marker = ch;
            let start = i;
            let mut j = i + 1;
            while j < len {
                let c = chars[j];
                if should_break(marker, c) {
                    break;
                }
                j += 1;
            }
            tokens.push((start, j));
            i = j;
            continue;
        }
        i += 1;
    }
    tokens
}

fn should_break(marker: char, ch: char) -> bool {
    if ch.is_whitespace() {
        return true;
    }
    match marker {
        '@' => matches!(
            ch,
            ')' | '(' | '{' | '}' | '[' | ']' | ',' | ';' | ':' | '"' | '\'' | '`'
        ),
        '/' => matches!(
            ch,
            ')' | '(' | '{' | '}' | '[' | ']' | ',' | ';' | ':' | '"' | '\'' | '`'
        ),
        _ => false,
    }
}
fn current_token_any(app: &App) -> Option<TokenInfo> {
    current_command_token(app).or_else(|| current_file_token(app))
}
