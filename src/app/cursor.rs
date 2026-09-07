//! Moving the cursor and keeping the view scrolled around it.

use super::*;

impl App {
    pub fn clamp(&mut self) {
        if self.view.is_empty() {
            self.cur = 0;
            self.top = 0;
            return;
        }
        self.cur = self.cur.min(self.view.len() - 1);
        self.scroll_to_cursor();
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.view.is_empty() {
            return;
        }
        let last = self.view.len() as isize - 1;
        self.cur = (self.cur as isize + delta).clamp(0, last) as usize;
        self.scroll_to_cursor();
    }

    pub fn goto(&mut self, row: usize) {
        if self.view.is_empty() {
            return;
        }
        self.cur = row.min(self.view.len() - 1);
        self.scroll_to_cursor();
    }

    /// Keeps the viewport around the cursor, honouring [`SCROLLOFF`].
    pub fn scroll_to_cursor(&mut self) {
        let h = self.track_h.max(1);
        let pad = SCROLLOFF.min(h.saturating_sub(1) / 2);
        if self.cur < self.top + pad {
            self.top = self.cur.saturating_sub(pad);
        }
        if self.cur + pad >= self.top + h {
            self.top = self.cur + pad + 1 - h;
        }
        let max_top = self.view.len().saturating_sub(h);
        self.top = self.top.min(max_top);
    }

    /// `zz`, `zt`, `zb`: move the view, not the cursor.
    pub fn scroll_cursor_to(&mut self, where_: char) {
        let h = self.track_h.max(1);
        self.top = match where_ {
            't' => self.cur,
            'b' => self.cur + 1 - h.min(self.cur + 1),
            _ => self.cur.saturating_sub(h / 2),
        };
        let max_top = self.view.len().saturating_sub(h);
        self.top = self.top.min(max_top);
    }

    /// `H`, `M`, `L`: move the cursor inside the visible window.
    pub fn cursor_to_screen(&mut self, where_: char) {
        let h = self.track_h.max(1);
        let last_visible = (self.top + h - 1).min(self.view.len().saturating_sub(1));
        let row = match where_ {
            'H' => (self.top + SCROLLOFF.min(h / 2)).min(last_visible),
            'L' => last_visible
                .saturating_sub(SCROLLOFF.min(h / 2))
                .max(self.top),
            _ => (self.top + last_visible) / 2,
        };
        self.cur = row;
        self.scroll_to_cursor();
    }

    /// `j`, `k`, `ctrl-d` and friends with the lyrics pane focused: the words
    /// move and nothing is selected, since a lyric is not a row you act on.
    ///
    /// Scrolling by hand pins the pane, because a page that scrolls itself
    /// back under you while you are reading it is worse than one that stays
    /// where you put it. `ctrl-w` off the pane, or the next track, lets it
    /// follow the song again.
    pub fn scroll_lyrics(&mut self, delta: isize) {
        self.lyrics_pinned = true;
        let max_top = self.lyrics_rows.saturating_sub(self.lyrics_h.max(1));
        self.lyrics_top = (self.lyrics_top as isize + delta).clamp(0, max_top as isize) as usize;
    }

    /// `gg` and `G` in the lyrics pane.
    pub fn lyrics_goto_end(&mut self, end: bool) {
        self.lyrics_pinned = true;
        self.lyrics_top = if end {
            self.lyrics_rows.saturating_sub(self.lyrics_h.max(1))
        } else {
            0
        };
    }

    /// Back to following the song, and back to wherever that puts it.
    pub fn unpin_lyrics(&mut self) {
        self.lyrics_pinned = false;
    }

    /// Moves the cursor in the folder list. The track pane does not follow: a
    /// folder is opened with enter, the same as a playlist.
    pub fn move_folder(&mut self, delta: isize) {
        let last = self.folders.len() as isize; // row 0 is the whole library
        self.folder_cur = (self.folder_cur as isize + delta).clamp(0, last) as usize;
        let h = self.folder_h.max(1);
        if self.folder_cur < self.folder_top {
            self.folder_top = self.folder_cur;
        }
        if self.folder_cur >= self.folder_top + h {
            self.folder_top = self.folder_cur + 1 - h;
        }
    }

    /// Enter on a folder row: this is what actually changes the track pane.
    pub fn open_folder(&mut self) {
        if self.jump_to_open(None, self.folder_cur) {
            return;
        }
        self.playlist_view = None;
        self.folder_open = self.folder_cur;
        self.cur = 0;
        self.top = 0;
        self.rebuild_view();
        self.focus = Pane::Tracks;
    }
}
