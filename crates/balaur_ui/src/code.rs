//! The editable code buffer: `ui.code_editor` and the calls that read and
//! move its caret, so a script can put a completion list or a hover card
//! where the text is and accept a completion into it.

use anyhow::{anyhow, Result};
use balaur_core::Engine;
use balaur_script::Value;
use egui::text::{CCursor, CCursorRange};
use egui::{vec2, Align, Color32, FontId};

use crate::bridge::{scale, with_ctx, with_ui};
use crate::theme;
use crate::widgets::{highlight, sc, syntax_for, Gutter, Marks, Opts, SyntaxColors};
use crate::UiState;

/// Where the caret or the pointer is in a code editor: a 1-based line, a
/// 1-based character column, and that spot on screen in design pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CodePosition {
    pub line: usize,
    pub column: usize,
    pub x: f32,
    pub y: f32,
    pub height: f32,
}

/// One run a language server coloured: a line, a column, a length and the
/// colour its kind maps to.
struct TokenRun {
    line: usize,
    column: usize,
    length: usize,
    color: Color32,
}

/// The `tokens` option: highlighting decided by a language server rather
/// than by the line tokenizer, as `{ line, column, length, kind, modifiers }`.
struct Tokens(Vec<TokenRun>);

impl Tokens {
    fn from_opts(opts: &Opts, colors: &SyntaxColors) -> Option<Self> {
        let items = opts.list("tokens")?;
        let mut runs: Vec<TokenRun> = items
            .iter()
            .filter_map(|item| {
                let Value::Map(fields) = item else {
                    return None;
                };
                let field = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v);
                let int = |name: &str| match field(name) {
                    Some(Value::Int(i)) => usize::try_from(*i).ok(),
                    Some(Value::Num(n)) if *n >= 0.0 => Some(*n as usize),
                    _ => None,
                };
                let kind = match field("kind") {
                    Some(Value::Str(s)) => s.as_str(),
                    _ => "",
                };
                let header = matches!(field("modifiers"), Some(Value::List(mods))
                    if mods.iter().any(|m| matches!(m, Value::Str(s) if s == "header")));
                Some(TokenRun {
                    line: int("line")?,
                    column: int("column")?,
                    length: int("length")?,
                    color: colors.token(kind, header),
                })
            })
            .collect();
        runs.sort_by_key(|r| (r.line, r.column));
        Some(Self(runs))
    }
}

/// Highlighting from `tokens`: every character a run covers takes the run's
/// colour, everything else the identifier colour, line by line.
fn highlight_tokens(
    text_src: &str,
    tokens: &Tokens,
    font: &FontId,
    colors: &SyntaxColors,
    marks: &Marks,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let fmt = |color: Color32, underline: egui::Stroke| egui::TextFormat {
        font_id: font.clone(),
        color,
        underline,
        ..Default::default()
    };
    let mut runs = tokens.0.iter().peekable();
    for (i, line) in text_src.split('\n').enumerate() {
        let n = i + 1;
        if i > 0 {
            job.append("\n", 0.0, fmt(colors.ident(), egui::Stroke::NONE));
        }
        let underline = marks.color(n).map_or(egui::Stroke::NONE, |color| {
            egui::Stroke::new(sc(1.0), color)
        });
        let chars: Vec<char> = line.chars().collect();
        let mut per_char = vec![colors.ident(); chars.len()];
        while runs.peek().is_some_and(|r| r.line < n) {
            runs.next();
        }
        while let Some(run) = runs.next_if(|r| r.line == n) {
            let start = run.column.saturating_sub(1).min(chars.len());
            let end = (start + run.length).min(chars.len());
            per_char[start..end].fill(run.color);
        }
        let mut start = 0;
        while start < chars.len() {
            let color = per_char[start];
            let end = per_char[start..]
                .iter()
                .position(|c| *c != color)
                .map_or(chars.len(), |k| start + k);
            let span: String = chars[start..end].iter().collect();
            job.append(&span, 0.0, fmt(color, underline));
            start = end;
        }
    }
    job
}

/// The 1-based line and character column of character `index` in `text`.
fn position_of(text: &str, index: usize) -> (usize, usize) {
    let (mut line, mut column) = (1, 1);
    for c in text.chars().take(index) {
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// The character index of a 1-based line and column in `text`, clamped to
/// the line's end and to the text's end.
fn index_of(text: &str, line: usize, column: usize) -> usize {
    let mut index = 0;
    for (n, row) in text.split('\n').enumerate() {
        let chars = row.chars().count();
        if n + 1 == line {
            return index + column.saturating_sub(1).min(chars);
        }
        index += chars + 1;
    }
    text.chars().count()
}

/// Returns the buffer, whether it changed, and the gutter line clicked this
/// frame, if any: `breakpoints` marks lines, `current_line` highlights one,
/// `tokens` colours the text from a language server instead of the tokenizer.
pub(crate) fn code_editor(
    eng: &Engine,
    id: &str,
    source: &str,
    opts: &Opts,
) -> Result<(String, bool, Option<i64>)> {
    // `language` overrides; otherwise highlight whatever the project is
    // written in, so an editor shows Rune as Rune.
    let language = opts.string("language").unwrap_or_else(|| {
        eng.try_resource::<balaur_core::project::ProjectManifest>()
            .map_or_else(|| "rune".to_string(), |m| m.borrow().language.clone())
    });
    let syntax = syntax_for(&language);
    let state = eng.resource::<UiState>();
    let mut buffer = {
        let cached = state.borrow().text_buffers.get(id).cloned();
        if let Some(b) = cached {
            b
        } else {
            state
                .borrow_mut()
                .text_buffers
                .insert(id.to_string(), source.to_string());
            source.to_string()
        }
    };
    let reveal = state.borrow_mut().code_reveal.remove(id);
    let size = opts.px("size", 12.5);
    let gutter = Gutter::from_opts(opts, size);
    let colors = SyntaxColors::from_opts(opts);
    let marks = Marks::from_opts(opts);
    let tokens = Tokens::from_opts(opts, &colors);
    let font = FontId::new(size, theme::family("mono"));
    let drawn = with_ui(|ui| {
        let font = font.clone();
        let row_h = ui
            .painter()
            .layout_no_wrap("0".into(), font.clone(), gutter.color)
            .size()
            .y;
        let mut drawn = Drawn::default();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let n_lines = buffer.split('\n').count().max(1);
            drawn.clicked = gutter.paint(ui, n_lines, row_h);
            ui.add_space(sc(12.0));
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                let mut job = match &tokens {
                    Some(tokens) => highlight_tokens(buf.as_str(), tokens, &font, &colors, &marks),
                    None => highlight(buf.as_str(), syntax, &font, &colors, &marks),
                };
                job.wrap.max_width = f32::INFINITY;
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let output = egui::TextEdit::multiline(&mut buffer)
                .id(egui::Id::new(id.to_string()))
                .frame(egui::Frame::NONE)
                .desired_width(ui.available_width())
                .layouter(&mut layouter)
                .show(ui);
            let response = &output.response.response;
            drawn.changed = response.changed();
            let place = |cursor: CCursor| {
                let rect = output
                    .galley
                    .pos_from_cursor(cursor)
                    .translate(output.galley_pos.to_vec2());
                let (line, column) = position_of(&buffer, cursor.index.0);
                CodePosition {
                    line,
                    column,
                    x: rect.min.x / scale(),
                    y: rect.min.y / scale(),
                    height: rect.height() / scale(),
                }
            };
            if response.has_focus() {
                drawn.caret = output.cursor_range.map(|r| place(r.primary));
            }
            if response.hovered() {
                drawn.pointer = ui
                    .input(|i| i.pointer.hover_pos())
                    .map(|pos| place(output.galley.cursor_from_pos(pos - output.galley_pos)));
            }
            if reveal {
                if let Some(range) = output.cursor_range {
                    let rect = output
                        .galley
                        .pos_from_cursor(range.primary)
                        .translate(output.galley_pos.to_vec2());
                    ui.scroll_to_rect(rect.expand2(vec2(0.0, row_h * 3.0)), Some(Align::Center));
                }
            }
        });
        Ok(drawn)
    })?;
    let mut state = state.borrow_mut();
    state.text_buffers.insert(id.to_string(), buffer.clone());
    match drawn.caret {
        Some(caret) => state.code_carets.insert(id.to_string(), caret),
        None => state.code_carets.remove(id),
    };
    match drawn.pointer {
        Some(pointer) => state.code_pointers.insert(id.to_string(), pointer),
        None => state.code_pointers.remove(id),
    };
    Ok((buffer, drawn.changed, drawn.clicked))
}

/// What one draw of the editor found out.
#[derive(Default)]
struct Drawn {
    changed: bool,
    clicked: Option<i64>,
    caret: Option<CodePosition>,
    pointer: Option<CodePosition>,
}

/// Replace `from..to` (1-based lines and columns) in editor `id` with `text`,
/// put the caret after it, focus the editor and scroll the caret into view
/// on the next draw. Returns the new buffer.
pub(crate) fn code_edit(
    eng: &Engine,
    id: &str,
    from: (usize, usize),
    to: (usize, usize),
    text: &str,
) -> Result<String> {
    let state = eng.resource::<UiState>();
    let buffer = state
        .borrow()
        .text_buffers
        .get(id)
        .cloned()
        .ok_or_else(|| anyhow!("no code editor is drawn as {id}"))?;
    let a = index_of(&buffer, from.0, from.1);
    let b = index_of(&buffer, to.0, to.1);
    let (a, b) = (a.min(b), a.max(b));
    let chars: Vec<char> = buffer.chars().collect();
    let mut edited: String = chars[..a].iter().collect();
    edited.push_str(text);
    edited.extend(chars[b..].iter());
    let caret = a + text.chars().count();
    {
        let mut state = state.borrow_mut();
        state.text_buffers.insert(id.to_string(), edited.clone());
        state.code_reveal.insert(id.to_string());
    }
    with_ctx(|ctx| {
        let widget = egui::Id::new(id.to_string());
        let mut edit_state = egui::text_edit::TextEditState::load(ctx, widget).unwrap_or_default();
        edit_state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(caret))));
        edit_state.store(ctx, widget);
        ctx.memory_mut(|m| m.request_focus(widget));
        Ok(())
    })?;
    Ok(edited)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_and_indices_are_inverses_within_a_line() {
        let text = "ab\ncd\n";
        assert_eq!(position_of(text, 4), (2, 2));
        assert_eq!(index_of(text, 2, 2), 4);
        assert_eq!(index_of(text, 2, 99), 5);
        assert_eq!(index_of(text, 9, 1), 6);
    }
}
