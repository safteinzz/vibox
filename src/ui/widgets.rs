//! Pane furniture with no idea what it is drawing: the window box, scrollbars,
//! truncation and the shared styles. Nothing here knows what a track or a
//! playlist is.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{
    Block, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use ratatui::text::Line;

/// Where a window landed: the whole box, for the scrollbars on its border, and
/// the rows its body may fill above the key row.
pub(super) struct Window {
    pub frame: Rect,
    pub body: Rect,
}

fn cells(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// Two borders and two cells of padding either side of the body.
const WINDOW_CHROME_W: usize = 6;

fn window_width(area: Rect, body_w: usize) -> u16 {
    cells(body_w.saturating_add(WINDOW_CHROME_W))
        .clamp(24, 88)
        .min(area.width)
}

/// How wide the body of a `window` asking for `body_w` comes out in `area`, for
/// a body that has to be wrapped before its height is known.
pub(super) fn window_body_width(area: Rect, body_w: usize) -> usize {
    usize::from(window_width(area, body_w)).saturating_sub(WINDOW_CHROME_W)
}

/// Draws a read only window: cyan, `title` on the top border and nothing else
/// there, `keys` dim on the last row after a blank one.
///
/// `body_w` and `body_h` are the body in cells, chrome excluded. The box is 24
/// to 88 wide and capped at `area`, never at a fraction of it, so a body taller
/// than `area` scrolls inside `Window::body` while the key row stays on screen.
pub(super) fn window(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    keys: &str,
    body_w: usize,
    body_h: usize,
) -> Window {
    let w = window_width(area, body_w);
    // The blank and the key row under the body, two borders and the top padding.
    let h = cells(body_h.saturating_add(5)).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };

    let block = Block::bordered()
        .title(format!(" {title} "))
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::new(2, 2, 1, 0));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [body, _, key_row] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(Line::styled(keys, dim().add_modifier(Modifier::DIM))),
        key_row,
    );

    Window { frame: popup, body }
}

/// A scrollbar on `area`'s right border for `total` rows, `view` of them on
/// screen from `top`, where `area` is the bordered rect it sits on. Nothing is
/// drawn when every row fits.
pub(super) fn vscrollbar(frame: &mut Frame, area: Rect, total: usize, top: usize, view: usize) {
    if total <= view {
        return;
    }
    let mut state = ScrollbarState::new(total - view).position(top);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None);
    frame.render_stateful_widget(bar, area.inner(Margin::new(0, 1)), &mut state);
}

/// A scrollbar on `area`'s bottom border for content `total` columns wide,
/// `view` of them on screen from `left`, kept one column off each corner.
/// Nothing is drawn when every column fits.
pub(super) fn hscrollbar(frame: &mut Frame, area: Rect, total: usize, left: usize, view: usize) {
    if total <= view {
        return;
    }
    let mut state = ScrollbarState::new(total - view).position(left);
    let bar = Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
        .begin_symbol(None)
        .end_symbol(None)
        .thumb_symbol("■");
    frame.render_stateful_widget(bar, area.inner(Margin::new(1, 0)), &mut state);
}

/// `vscrollbar` down the last column of `rows`, for a pane with no top or
/// bottom border of its own. `rows` must not start on the screen's first row.
pub(super) fn pane_vscrollbar(frame: &mut Frame, rows: Rect, total: usize, top: usize) {
    // `vscrollbar` keeps off a border row at each end, so it gets one more
    // row each way than the pane has.
    let around = Rect {
        y: rows.y.saturating_sub(1),
        height: rows.height.saturating_add(2),
        ..rows
    };
    vscrollbar(frame, around, total, top, rows.height as usize);
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
