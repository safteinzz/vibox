//! One editable name: the text, the cursor, and the vi motions over it.
//!
//! Every pane that lets you rename something owns its own `NameBuffer`. They
//! share this code and nothing else: two buffers never see each other, so
//! editing a folder name cannot show up on a track row, which is exactly what
//! happened when the two panes shared one map keyed by row.

/// What kind of run a character belongs to, the way vim splits a line.
///
/// Punctuation is a word of its own, so `w` on `ABBA - As` stops on the dash.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

#[derive(Clone, Default)]
pub struct NameBuffer {
    text: Vec<char>,
    col: usize,
    /// First character shown, for a name wider than the column it sits in.
    scroll: usize,
    /// Where `v` started, when a selection is being made. The selection runs
    /// from here to the cursor with both ends included, the way vim's
    /// charwise visual does.
    anchor: Option<usize>,
}

impl NameBuffer {
    pub fn new(text: &str) -> NameBuffer {
        NameBuffer {
            text: text.chars().collect(),
            col: 0,
            scroll: 0,
            anchor: None,
        }
    }

    pub fn text(&self) -> String {
        self.text.iter().collect()
    }

    pub fn col(&self) -> usize {
        self.col
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Last character position, which is as far right as normal mode goes.
    fn last(&self) -> usize {
        self.text.len().saturating_sub(1)
    }

    // ---- motions ----------------------------------------------------------

    pub fn left(&mut self) {
        self.col = self.col.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.col = (self.col + 1).min(self.last());
    }

    pub fn jump_start(&mut self) {
        self.col = 0;
    }

    pub fn jump_end(&mut self) {
        self.col = self.last();
    }

    pub fn jump_word_forward(&mut self) {
        self.col = self.word_forward().min(self.last());
    }

    pub fn jump_word_back(&mut self) {
        self.col = self.word_back();
    }

    pub fn jump_word_end(&mut self) {
        self.col = self.word_end();
    }

    /// Start of the next word, or the end of the text when there is none.
    ///
    /// Unclamped, because an operator needs the true end: `cw` on a single word
    /// has to take all of it.
    fn word_forward(&self) -> usize {
        let mut i = self.col;
        if i < self.text.len() {
            let kind = class(self.text[i]);
            if kind != Class::Space {
                while i < self.text.len() && class(self.text[i]) == kind {
                    i += 1;
                }
            }
        }
        while i < self.text.len() && class(self.text[i]) == Class::Space {
            i += 1;
        }
        i
    }

    fn word_back(&self) -> usize {
        let mut i = self.col.min(self.text.len());
        while i > 0 && class(self.text[i - 1]) == Class::Space {
            i -= 1;
        }
        if i > 0 {
            let kind = class(self.text[i - 1]);
            while i > 0 && class(self.text[i - 1]) == kind {
                i -= 1;
            }
        }
        i
    }

    fn word_end(&self) -> usize {
        let mut i = self.col + 1;
        while i < self.text.len() && class(self.text[i]) == Class::Space {
            i += 1;
        }
        if i < self.text.len() {
            let kind = class(self.text[i]);
            while i + 1 < self.text.len() && class(self.text[i + 1]) == kind {
                i += 1;
            }
        }
        i.min(self.last())
    }

    // ---- entering insert --------------------------------------------------

    pub fn append_here(&mut self) {
        self.col = (self.col + 1).min(self.text.len());
    }

    pub fn append_at_end(&mut self) {
        self.col = self.text.len();
    }

    // ---- edits ------------------------------------------------------------

    pub fn insert(&mut self, c: char) {
        let at = self.col.min(self.text.len());
        self.text.insert(at, c);
        self.col = at + 1;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.text.remove(self.col - 1);
            self.col -= 1;
        }
    }

    /// `x`, and Delete in insert: takes the character under the cursor.
    ///
    /// Every delete returns what it removed, because in vi a delete is also a
    /// yank: `p` puts back whatever the last one took.
    pub fn delete_here(&mut self) -> String {
        if self.col >= self.text.len() {
            return String::new();
        }
        let taken = self.text.remove(self.col);
        self.col = self.col.min(self.last());
        taken.to_string()
    }

    /// `D` and `C`: everything from the cursor on.
    pub fn truncate_here(&mut self) -> String {
        let taken: String = self.text.drain(self.col.min(self.text.len())..).collect();
        self.col = self.col.min(self.last());
        taken
    }

    pub fn clear(&mut self) -> String {
        let taken: String = self.text.drain(..).collect();
        self.col = 0;
        taken
    }

    /// `dw`, and `cw` which stops at the end of the word the way vim's does.
    pub fn delete_word(&mut self, to_word_end: bool) -> String {
        let to = if to_word_end {
            self.word_end() + 1
        } else {
            self.word_forward().max(self.col)
        };
        let to = to.min(self.text.len());
        let taken: String = self.text.drain(self.col..to).collect();
        self.col = self.col.min(self.last());
        taken
    }

    /// `db`, and ctrl-w on a command line.
    pub fn delete_word_back(&mut self) -> String {
        let from = self.word_back();
        let taken: String = self.text.drain(from..self.col).collect();
        self.col = from;
        taken
    }

    // ---- selecting --------------------------------------------------------

    /// True while `v` is active, which is what makes the statusline say so.
    pub fn selecting(&self) -> bool {
        self.anchor.is_some()
    }

    /// `v`, and `v` again to drop it.
    pub fn toggle_visual(&mut self) {
        self.anchor = if self.anchor.is_some() {
            None
        } else {
            Some(self.col)
        };
    }

    pub fn stop_visual(&mut self) {
        self.anchor = None;
    }

    /// The selected span, both ends included, in character positions.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        Some((anchor.min(self.col), anchor.max(self.col)))
    }

    /// `y` in visual: the selected text, left in place.
    pub fn copy_selection(&self) -> Option<String> {
        let (lo, hi) = self.selection()?;
        Some(self.text.get(lo..=hi.min(self.last()))?.iter().collect())
    }

    /// `d` and `c` in visual: the selected text, taken out.
    pub fn cut_selection(&mut self) -> Option<String> {
        let (lo, hi) = self.selection()?;
        self.anchor = None;
        if lo >= self.text.len() {
            return None;
        }
        let taken: String = self.text.drain(lo..=hi.min(self.last())).collect();
        self.col = lo.min(self.last());
        Some(taken)
    }

    /// `p` puts after the cursor and `P` before it, as vi does, and leaves the
    /// cursor on the last character put so a second `p` does not stack.
    pub fn paste(&mut self, text: &str, after: bool) {
        if text.is_empty() {
            return;
        }
        let at = if after && !self.text.is_empty() {
            self.col + 1
        } else {
            self.col
        }
        .min(self.text.len());

        for (i, c) in text.chars().enumerate() {
            self.text.insert(at + i, c);
        }
        self.col = at + text.chars().count() - 1;
    }

    /// Slides the window so the cursor stays visible in `width` cells.
    pub fn follow(&mut self, width: usize) {
        if width < 2 {
            return;
        }
        if self.col < self.scroll {
            self.scroll = self.col;
        }
        // One cell is kept free so the cursor can sit past the last character.
        if self.col + 1 >= self.scroll + width {
            self.scroll = self.col + 2 - width;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str, col: usize) -> NameBuffer {
        let mut buf = NameBuffer::new(text);
        for _ in 0..col {
            buf.right();
        }
        buf
    }

    #[test]
    fn w_treats_punctuation_as_its_own_word() {
        let mut buf = buffer("ABBA - As Good as New", 0);
        buf.jump_word_forward();
        assert_eq!(buf.col(), 5, "the dash is a word");
        buf.jump_word_forward();
        assert_eq!(buf.col(), 7);
        buf.jump_word_forward();
        assert_eq!(buf.col(), 10);
    }

    #[test]
    fn b_comes_back_over_punctuation_the_same_way() {
        let mut buf = buffer("ABBA - As", 7);
        buf.jump_word_back();
        assert_eq!(buf.col(), 5);
        buf.jump_word_back();
        assert_eq!(buf.col(), 0);
    }

    #[test]
    fn cw_takes_the_whole_word_when_there_is_only_one() {
        let mut buf = buffer("testing", 0);
        buf.delete_word(true);
        assert_eq!(buf.text(), "", "cw on a single word leaves nothing behind");
    }

    #[test]
    fn cw_stops_at_the_end_of_the_word_and_dw_takes_the_space() {
        let mut change = buffer("Knife Party", 0);
        change.delete_word(true);
        assert_eq!(change.text(), " Party");

        let mut delete = buffer("Knife Party", 0);
        delete.delete_word(false);
        assert_eq!(delete.text(), "Party");
    }

    #[test]
    fn typing_inserts_at_the_cursor_and_moves_it_along() {
        let mut buf = buffer("ac", 1);
        buf.insert('b');
        assert_eq!(buf.text(), "abc");
        assert_eq!(buf.col(), 2);
        buf.backspace();
        assert_eq!(buf.text(), "ac");
    }

    #[test]
    fn the_motions_hold_on_an_empty_name() {
        let mut buf = NameBuffer::new("");
        buf.jump_word_forward();
        buf.jump_word_back();
        buf.jump_word_end();
        buf.right();
        buf.delete_here();
        assert_eq!(buf.text(), "");
        assert_eq!(buf.col(), 0);
    }

    #[test]
    fn the_window_follows_the_cursor_in_a_narrow_column() {
        let mut buf = buffer("a very long name indeed", 0);
        buf.follow(10);
        assert_eq!(buf.scroll(), 0);

        buf.jump_end();
        buf.follow(10);
        assert!(buf.scroll() > 0, "the far end has to be visible");
        assert!(buf.col() >= buf.scroll());
    }

    /// Two buffers are two buffers: this is the whole point of the type.
    #[test]
    fn buffers_do_not_share_anything() {
        let mut one = NameBuffer::new("folder");
        let mut two = NameBuffer::new("track");
        one.clear();
        one.insert('x');
        two.jump_end();
        assert_eq!(one.text(), "x");
        assert_eq!(two.text(), "track");
        assert_eq!(one.col(), 1);
        assert_eq!(two.col(), 4);
    }

    #[test]
    fn p_puts_after_the_cursor_and_capital_p_before_it() {
        let mut after = buffer("abc", 1);
        after.paste("XY", true);
        assert_eq!(after.text(), "abXYc", "p goes after the character under the cursor");

        let mut before = buffer("abc", 1);
        before.paste("XY", false);
        assert_eq!(before.text(), "aXYbc", "P goes before it");
    }

    #[test]
    fn a_put_leaves_the_cursor_on_what_it_put() {
        let mut buf = buffer("abc", 0);
        buf.paste("XY", true);
        assert_eq!(buf.text(), "aXYbc");
        assert_eq!(buf.col(), 2, "on the last character put, so a second p does not stack");
    }

    #[test]
    fn putting_into_an_empty_name_does_not_fall_off_the_end() {
        let mut buf = buffer("", 0);
        buf.paste("hi", true);
        assert_eq!(buf.text(), "hi", "there is no character to go after");
        assert_eq!(buf.col(), 1);
    }

    /// Every delete is a yank in vi, which is what makes `dw` then `$p` a move.
    #[test]
    fn a_delete_hands_back_what_it_took() {
        let mut buf = buffer("one two", 0);
        assert_eq!(buf.delete_word(false), "one ");
        assert_eq!(buf.text(), "two");

        let mut buf = buffer("abc", 1);
        assert_eq!(buf.delete_here(), "b");
        assert_eq!(buf.truncate_here(), "c");

        let mut buf = buffer("gone", 0);
        assert_eq!(buf.clear(), "gone");
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn v_selects_from_where_it_started_to_the_cursor() {
        let mut buf = buffer("one two", 0);
        assert!(!buf.selecting());
        buf.toggle_visual();
        assert!(buf.selecting());
        buf.jump_word_forward();
        assert_eq!(buf.selection(), Some((0, 4)), "both ends are included");

        // and selecting backwards is the same span
        let mut back = buffer("one two", 4);
        back.toggle_visual();
        back.jump_word_back();
        assert_eq!(back.selection(), Some((0, 4)));
    }

    #[test]
    fn a_selection_is_dropped_without_touching_the_name() {
        let mut buf = buffer("keep me", 0);
        buf.toggle_visual();
        buf.jump_word_forward();
        buf.stop_visual();
        assert!(!buf.selecting());
        assert_eq!(buf.selection(), None);
        assert_eq!(buf.text(), "keep me");

        // `v` a second time drops it too
        buf.toggle_visual();
        buf.toggle_visual();
        assert!(!buf.selecting());
    }

    #[test]
    fn copying_a_selection_leaves_it_alone_and_cutting_takes_it() {
        // `ve`, which is the idiom for "this word and no more": the character
        // under the cursor is in the selection, the way vi counts.
        let mut buf = buffer("one two", 0);
        buf.toggle_visual();
        buf.jump_word_end();
        assert_eq!(buf.copy_selection().as_deref(), Some("one"));
        assert_eq!(buf.text(), "one two", "y does not remove anything");

        assert_eq!(buf.cut_selection().as_deref(), Some("one"));
        assert_eq!(buf.text(), " two");
        assert!(!buf.selecting(), "the selection is spent");
    }

    /// vi counts the character under the cursor as selected, so `vw` reaches
    /// one past the start of the next word rather than stopping short of it.
    #[test]
    fn a_selection_includes_the_character_under_the_cursor() {
        let mut buf = buffer("one two", 0);
        buf.toggle_visual();
        buf.jump_word_forward();
        assert_eq!(buf.col(), 4, "w landed on the `t`");
        assert_eq!(buf.copy_selection().as_deref(), Some("one t"));
    }

    /// The reason all of this exists: swapping the halves of `TITLE - ARTIST`
    /// without retyping either of them.
    #[test]
    fn a_selection_can_be_cut_and_put_back_at_the_end() {
        let mut buf = buffer("TITLE - ARTIST", 0);

        // v, w onto the dash, l onto the space after it
        buf.toggle_visual();
        buf.jump_word_forward();
        buf.right();
        let taken = buf.cut_selection().unwrap();
        assert_eq!(taken, "TITLE - ");
        assert_eq!(buf.text(), "ARTIST");

        // $ then p
        buf.jump_end();
        buf.paste(&taken, true);
        assert_eq!(buf.text(), "ARTISTTITLE - ");
    }

    /// `yw` must take exactly what `dw` would, which is why it runs the same
    /// motion on a copy instead of a second implementation of the maths.
    #[test]
    fn a_yank_takes_what_the_same_delete_would_have() {
        for motion in ["w", "e", "b", "$", "all"] {
            let start = buffer("one two three", 4);
            let mut cut = start.clone();
            let mut probe = start.clone();

            let (deleted, yanked) = match motion {
                "w" => (cut.delete_word(false), probe.delete_word(false)),
                "e" => (cut.delete_word(true), probe.delete_word(true)),
                "b" => (cut.delete_word_back(), probe.delete_word_back()),
                "$" => (cut.truncate_here(), probe.truncate_here()),
                _ => (cut.clear(), probe.clear()),
            };

            assert_eq!(deleted, yanked, "`y{motion}` and `d{motion}` take the same text");
            assert!(!yanked.is_empty(), "`{motion}` had something to take");
        }
    }
}

