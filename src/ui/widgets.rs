//! Pane furniture with no idea what it is drawing: scrollbars, truncation and
//! the shared styles. Nothing here knows what a track or a playlist is.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Paragraph;

use ratatui::text::Line;

/// A thumb along the bottom border showing how much is off to the sides.
/// The same down the right border, for a list taller than the popup.
///
/// Without it a batch that scrolls looks like a batch that is simply cut off,
/// which is the wrong thing to believe about a list of things `:w` will do.
pub(super) fn draw_vscrollbar(
    frame: &mut Frame,
    popup: Rect,
    total: usize,
    top: usize,
    view: usize,
) {
    let track = popup.height.saturating_sub(2) as usize;
    if track == 0 || total == 0 {
        return;
    }

    let thumb = (track * view / total).max(1).min(track);
    let at = if total > view {
        (track - thumb) * top / (total - view)
    } else {
        0
    };

    let column: Vec<Line> = (0..track)
        .map(|cell| {
            let glyph = if cell >= at && cell < at + thumb {
                "┃"
            } else {
                "│"
            };
            Line::styled(glyph, Style::default().fg(Color::Cyan))
        })
        .collect();

    let area = Rect {
        x: popup.x + popup.width.saturating_sub(1),
        y: popup.y + 1,
        width: 1,
        height: track as u16,
    };
    frame.render_widget(Paragraph::new(column), area);
}

pub(super) fn draw_hscrollbar(
    frame: &mut Frame,
    popup: Rect,
    total: usize,
    pan: usize,
    view: usize,
) {
    let track = popup.width.saturating_sub(2) as usize;
    if track == 0 || total == 0 {
        return;
    }

    let thumb = (track * view / total).max(1).min(track);
    let at = if total > view {
        (track - thumb) * pan / (total - view)
    } else {
        0
    };

    let mut bar = String::new();
    for cell in 0..track {
        bar.push(if cell >= at && cell < at + thumb {
            '━'
        } else {
            '─'
        });
    }

    let row = Rect {
        x: popup.x + 1,
        y: popup.y + popup.height.saturating_sub(1),
        width: track as u16,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::styled(bar, Style::default().fg(Color::Cyan))),
        row,
    );
}

pub(super) fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// The row being renamed, in any pane.
///
/// Deliberately not the reversed cursor style: reversing a row that already
/// carries a coloured cursor block hides the block, which is how the cursor
/// went missing in the sidebar.
pub(super) fn editing_style() -> Style {
    Style::default().bg(Color::Indexed(236))
}

/// Cursor line: reversed when the pane has focus, dimmed when it does not.
pub(super) fn cursor_style(focused: bool) -> Style {
    if focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().bg(Color::Indexed(236))
    }
}

/// Pads or cuts to an exact number of terminal cells.
///
/// Measured in display width, not characters: a cjk glyph occupies two cells,
/// so counting characters would push every column after it out of line.
pub(super) fn truncate(s: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    if width == 0 {
        return String::new();
    }
    let shown = s.width();
    if shown <= width {
        return format!("{s}{}", " ".repeat(width - shown));
    }

    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    used += 1;
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}
