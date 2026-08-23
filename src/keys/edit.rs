//! The rename buffer: vi editing of a name in place, its operators, counts and
//! word motions. Nothing here reaches the disk; `:w` is what writes.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Mode};

/// The list as a buffer of filenames: vi motions and operators on the name
/// under the cursor, `j` and `k` to move between rows, `:w` to write.
///
/// Whichever pane `c` opened hands over its own `NameBuffer`, and everything
/// below works on that. There is one implementation of the editing, not one
/// per pane.
pub(super) fn edit_mode(app: &mut App, key: KeyEvent) {
    if app.mode == Mode::EditInsert {
        edit_insert(app, key);
        return;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // A count, read before the pending operator rather than after it, so that
    // `3w`, `3dw` and `d3w` all mean what vi says they mean. `0` is the motion
    // to the front until a count has started, exactly as in vim.
    if !ctrl
        && let KeyCode::Char(c @ '0'..='9') = key.code
        && (c != '0' || app.count.is_some())
    {
        let digit = c as usize - '0' as usize;
        app.count = Some(app.count.unwrap_or(0) * 10 + digit);
        return;
    }

    // `c`, `d` and `y` wait for their motion.
    if let Some(op) = app.pending.take() {
        edit_operator(app, op, key);
        return;
    }

    // Keys that are about the pane rather than the name it is editing.
    match key.code {
        // Esc drops a selection first, the way vim does, so it takes two of
        // them to leave a name you were selecting inside.
        KeyCode::Esc if app.name_selecting() => {
            if let Some(buf) = app.name_buffer() {
                buf.stop_visual();
            }
            return;
        }
        // Leaving a name is not a write: it joins the pending set, like
        // marking a file does, and `:w` writes the lot. A half typed count
        // goes with it, or it would land on the next key in normal mode.
        KeyCode::Esc => {
            app.count = None;
            app.commit_name();
            app.mode = Mode::Normal;
            return;
        }
        KeyCode::Char(':') => {
            app.mode = Mode::Command;
            app.line_prefix = ':';
            app.line.clear();
            app.line_cur = 0;
            return;
        }
        KeyCode::Char('u') => {
            app.undo();
            return;
        }
        KeyCode::Char('r') if ctrl => {
            app.redo();
            return;
        }
        KeyCode::Char('j') | KeyCode::Down if app.renaming_a_track() => {
            app.edit_next_row(1);
            return;
        }
        KeyCode::Char('k') | KeyCode::Up if app.renaming_a_track() => {
            app.edit_next_row(-1);
            return;
        }
        // With a selection up these act on it instead of waiting for a motion,
        // which is what makes `vw...d` work; without one they are operators
        // and the next key is the motion, so `dw` and `yw` still read as vi.
        KeyCode::Char(op @ ('c' | 'd' | 'y')) if !app.name_selecting() => {
            app.pending = Some(op);
            return;
        }
        _ => {}
    }

    // Anything that changes the text is undoable on its own.
    if matches!(
        key.code,
        KeyCode::Char('i' | 'a' | 'I' | 'A' | 'x' | 'D' | 'C' | 'S' | 's' | 'c' | 'd' | 'p' | 'P')
            | KeyCode::Delete
    ) {
        app.checkpoint();
    }

    let insert = matches!(
        key.code,
        KeyCode::Char('i' | 'a' | 'I' | 'A' | 'C' | 'S' | 's' | 'c')
    );
    let register = app.name_reg.clone();
    let count = app.count.take().unwrap_or(1);
    let Some(buf) = app.name_buffer() else { return };

    // What this key took out of the name, if anything. A delete is a yank
    // too, so whatever comes back lands in the register for `p`.
    let mut taken = None;

    match key.code {
        KeyCode::Char('h') | KeyCode::Left => times(count, || buf.left()),
        KeyCode::Char('l') | KeyCode::Right => times(count, || buf.right()),
        KeyCode::Char('0') => buf.jump_start(),
        KeyCode::Char('^') => buf.jump_first_nonblank(),
        KeyCode::Char('$') => buf.jump_end(),
        KeyCode::Char('w') if !ctrl => times(count, || buf.jump_word_forward()),
        KeyCode::Char('b') => times(count, || buf.jump_word_back()),
        KeyCode::Char('e') => times(count, || buf.jump_word_end()),
        KeyCode::Char('v') => buf.toggle_visual(),
        KeyCode::Char('i') => {}
        KeyCode::Char('a') => buf.append_here(),
        // vi's `I` inserts at the first non-blank, not at column zero.
        KeyCode::Char('I') => buf.jump_first_nonblank(),
        KeyCode::Char('A') => buf.append_at_end(),
        // `s` substitutes the character(s) under the cursor: same take as `x`,
        // but it drops into insert like `cl` does.
        KeyCode::Char('x' | 's') | KeyCode::Delete => {
            taken = Some(gather(count, || buf.delete_here()));
        }
        KeyCode::Char('D') => taken = Some(buf.truncate_here()),
        KeyCode::Char('C') => taken = Some(buf.truncate_here()),
        KeyCode::Char('S') => taken = Some(buf.clear()),
        // Only reachable with a selection up: the guard above sends the bare
        // `c` and `d` off to wait for a motion instead.
        KeyCode::Char('c' | 'd') => taken = buf.cut_selection(),
        KeyCode::Char('y') => {
            taken = buf.copy_selection();
            buf.stop_visual();
        }
        KeyCode::Char(c @ ('p' | 'P')) => times(count, || buf.paste(&register, c == 'p')),
        _ => return,
    }

    if let Some(text) = taken.filter(|t| !t.is_empty()) {
        app.name_reg = text;
    }

    if insert {
        app.mode = Mode::EditInsert;
    }
}

/// Runs a motion `count` times, which is all a count means in vi.
pub(super) fn times(count: usize, mut motion: impl FnMut()) {
    for _ in 0..count {
        motion();
    }
}

/// The same for an edit, keeping everything it took so `3x` fills the register
/// with all three characters rather than only the last one.
pub(super) fn gather(count: usize, mut edit: impl FnMut() -> String) -> String {
    let mut all = String::new();
    for _ in 0..count {
        all.push_str(&edit());
    }
    all
}

/// Second key of `cw`, `cc`, `dw`, `dd`.
pub(super) fn edit_operator(app: &mut App, op: char, key: KeyEvent) {
    // `y` only reads, so it neither checkpoints nor touches the name: it runs
    // the motion on a copy and keeps what that would have taken. Whatever
    // `dw` deletes is by construction exactly what `yw` yanks.
    // `d3w` and `3dw` are the same thing, so the count is read here whichever
    // side of the operator it was typed on.
    let count = app.count.take().unwrap_or(1);

    if op == 'y' {
        let Some(buf) = app.name_buffer() else { return };
        let mut probe = buf.clone();
        let taken = match key.code {
            KeyCode::Char('y') => probe.clear(),
            KeyCode::Char('w' | 'e') => {
                gather(count, || probe.delete_word(key.code == KeyCode::Char('e')))
            }
            KeyCode::Char('b') => gather(count, || probe.delete_word_back()),
            KeyCode::Char('$') => probe.truncate_here(),
            _ => return,
        };
        if !taken.is_empty() {
            app.name_reg = taken;
        }
        return;
    }

    app.checkpoint();
    let Some(buf) = app.name_buffer() else { return };

    let taken = match (op, key.code) {
        ('c', KeyCode::Char('c')) | ('d', KeyCode::Char('d')) => buf.clear(),
        // vim's own quirk: `cw` changes to the end of the word, the way `ce`
        // does, instead of eating the space after it like `dw`.
        (_, KeyCode::Char('w' | 'e')) => gather(count, || {
            buf.delete_word(op == 'c' || key.code == KeyCode::Char('e'))
        }),
        (_, KeyCode::Char('b')) => gather(count, || buf.delete_word_back()),
        (_, KeyCode::Char('l')) => gather(count, || buf.delete_here()),
        (_, KeyCode::Char('h')) => gather(count, || buf.delete_back()),
        (_, KeyCode::Char('$')) => buf.truncate_here(),
        _ => return,
    };

    if !taken.is_empty() {
        app.name_reg = taken;
    }

    if op == 'c' {
        app.mode = Mode::EditInsert;
    }
}

pub(super) fn edit_insert(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => {
            if let Some(buf) = app.name_buffer() {
                buf.left();
            }
            app.mode = Mode::Edit;
            return;
        }
        KeyCode::Enter => {
            app.mode = Mode::Edit;
            return;
        }
        _ => {}
    }

    let Some(buf) = app.name_buffer() else { return };
    match key.code {
        KeyCode::Backspace => buf.backspace(),
        // Insert mode deletes do not fill the register: in vi only a normal
        // mode delete is also a yank.
        KeyCode::Delete => drop(buf.delete_here()),
        KeyCode::Left => buf.left(),
        KeyCode::Right => buf.append_here(),
        KeyCode::Home => buf.jump_start(),
        KeyCode::End => buf.append_at_end(),
        KeyCode::Char('u') if ctrl => drop(buf.clear()),
        KeyCode::Char('w') if ctrl => drop(buf.delete_word_back()),
        KeyCode::Char(c) => buf.insert(c),
        _ => {}
    }
}
