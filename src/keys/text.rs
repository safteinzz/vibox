//! vi text helpers shared by the rename buffer and the `:`/`/` lines: char
//! vectors, the character classes a word motion steps over, and where the word
//! under an offset starts.

use crate::app::App;

pub(super) fn chars(app: &App) -> Vec<char> {
    app.line.chars().collect()
}

/// What kind of run a character belongs to, the way vim splits a line: a word
/// of letters and digits, a run of punctuation, or whitespace between them.
///
/// This is why `w` on `ABBA - As` stops on the dash: punctuation is a word of
/// its own, not a separator to skip over.
#[derive(PartialEq, Eq, Clone, Copy)]
pub(super) enum Class {
    Space,
    Word,
    Punct,
}

pub(super) fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// Start of the word at or before `at`, which is where `b` and `ctrl-w` land.
pub(super) fn word_start(text: &[char], at: usize) -> usize {
    let mut i = at.min(text.len());
    while i > 0 && class(text[i - 1]) == Class::Space {
        i -= 1;
    }
    if i > 0 {
        let kind = class(text[i - 1]);
        while i > 0 && class(text[i - 1]) == kind {
            i -= 1;
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ctrl-w` on the command line, which is the only motion left in here now
    /// that names are edited through `NameBuffer`.
    #[test]
    fn ctrl_w_deletes_back_to_the_start_of_the_word() {
        let text: Vec<char> = "late night mix".chars().collect();
        assert_eq!(word_start(&text, text.len()), 11);
        assert_eq!(word_start(&text, 11), 5);
        assert_eq!(word_start(&text, 0), 0);
    }
}
