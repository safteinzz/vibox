//! Text helpers for the `:` and `/` lines: the line as characters, and where
//! the word under an offset starts.

use crate::app::App;
use crate::name::{Class, class};

pub(super) fn chars(app: &App) -> Vec<char> {
    app.line.chars().collect()
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
