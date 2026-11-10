//! Visual mode selection.

use std::path::PathBuf;

use super::*;

impl App {
    /// Inclusive row range under the cursor, or the whole visual selection.
    pub fn selection_range(&self) -> (usize, usize) {
        match self.visual_anchor {
            Some(anchor) if self.mode == Mode::Visual => {
                (anchor.min(self.cur), anchor.max(self.cur))
            }
            _ => (self.cur, self.cur),
        }
    }

    pub fn exit_visual(&mut self) {
        self.visual_anchor = None;
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
        }
    }

    /// `y`: remembers the selected tracks so `p` can put them in a playlist.
    pub fn yank_selection(&mut self) {
        if self.view.is_empty() {
            self.error("nothing here to yank");
            return;
        }
        let (a, b) = self.selection_range();
        let b = b.min(self.view.len() - 1);
        let paths: Vec<PathBuf> = self.view[a..=b]
            .iter()
            .map(|&i| self.tracks[i].path.clone())
            .collect();
        self.yank = self.register(&paths);
        // One register, as in vim: a cut from before is no longer what `p` puts.
        self.cut.clear();
        let n = self.yank.len();
        self.exit_visual();
        self.info(format!("yanked {n} track{}", plural(n)));
    }
}
