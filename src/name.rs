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
}

impl NameBuffer {
    pub fn new(text: &str) -> NameBuffer {
        NameBuffer {
            text: text.chars().collect(),
            col: 0,
            scroll: 0,
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
    pub fn delete_here(&mut self) {
        if self.col < self.text.len() {
            self.text.remove(self.col);
            self.col = self.col.min(self.last());
        }
    }

    /// `D` and `C`: everything from the cursor on.
    pub fn truncate_here(&mut self) {
        self.text.truncate(self.col);
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.col = 0;
    }

    /// `dw`, and `cw` which stops at the end of the word the way vim's does.
    pub fn delete_word(&mut self, to_word_end: bool) {
        let to = if to_word_end {
            self.word_end() + 1
        } else {
            self.word_forward().max(self.col)
        };
        let to = to.min(self.text.len());
        self.text.drain(self.col..to);
        self.col = self.col.min(self.last());
    }

    /// `db`, and ctrl-w on a command line.
    pub fn delete_word_back(&mut self) {
        let from = self.word_back();
        self.text.drain(from..self.col);
        self.col = from;
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
}
