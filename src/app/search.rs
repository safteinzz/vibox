//! `/` and `?`, matching against the columns of the row.




use super::*;

impl App {
    /// Vim smartcase: a lowercase pattern matches anything, an uppercase one
    /// is taken literally.
    pub fn search(&mut self, pattern: &str, backward: bool, from: usize) -> bool {
        if pattern.is_empty() || self.view.is_empty() {
            return false;
        }
        let smart = pattern.chars().any(char::is_uppercase);
        let needle = if smart {
            pattern.to_string()
        } else {
            pattern.to_lowercase()
        };
        let n = self.view.len();

        for step in 1..=n {
            let row = if backward {
                (from + n - (step % n)) % n
            } else {
                (from + step) % n
            };
            let hay = self.tracks[self.view[row]].haystack();
            let hay = if smart { hay } else { hay.to_lowercase() };
            if hay.contains(&needle) {
                self.goto(row);
                return true;
            }
        }
        false
    }
}
