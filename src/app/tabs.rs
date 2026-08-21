//! View tabs, each holding its own cursor, scroll and sort.

use super::*;

impl App {
    pub(super) fn snapshot(&self) -> ViewTab {
        ViewTab {
            playlist: self.playlist_view.clone(),
            folder: self.folder_open,
            cur: self.cur,
            top: self.top,
            sort_key: self.sort_key,
            rows: self.playlist_rows.clone(),
            dirty: self.playlist_dirty,
        }
    }

    fn restore(&mut self, tab: &ViewTab) {
        self.playlist_view = tab.playlist.clone();
        self.playlist_rows = tab.rows.clone();
        self.playlist_dirty = tab.dirty;
        self.folder_open = tab.folder;
        self.folder_cur = tab.folder;
        if self.sort_key != tab.sort_key {
            self.set_sort(tab.sort_key);
        }
        self.rebuild_view();
        self.cur = tab.cur.min(self.view.len().saturating_sub(1));
        self.top = tab.top;
        self.clamp();
    }

    /// Name for each tab, and whether it has unwritten changes.
    pub fn tab_labels(&self) -> Vec<(String, bool)> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let tab = if i == self.tab_idx {
                    &self.snapshot()
                } else {
                    tab
                };
                let name = match (&tab.playlist, tab.folder) {
                    (Some(name), _) => name.clone(),
                    (None, 0) => "everything".to_string(),
                    (None, folder) => self
                        .folders
                        .get(folder - 1)
                        .map_or_else(|| "everything".to_string(), |(label, _)| label.clone()),
                };
                (name, tab.dirty)
            })
            .collect()
    }

    pub fn cycle_tab(&mut self, delta: isize) {
        if self.tabs.len() < 2 {
            self.info("only one tab: `t` on a folder or playlist opens another");
            return;
        }
        self.tabs[self.tab_idx] = self.snapshot();
        let len = self.tabs.len() as isize;
        self.tab_idx = ((self.tab_idx as isize + delta).rem_euclid(len)) as usize;
        let tab = self.tabs[self.tab_idx].clone();
        self.restore(&tab);
    }

    /// Switches to the tab already showing this view, if one is.
    ///
    /// A view is only ever open once: two tabs of the same playlist would drift
    /// apart and the later `:w` would quietly win.
    pub(super) fn jump_to_open(&mut self, playlist: Option<&str>, folder: usize) -> bool {
        self.tabs[self.tab_idx] = self.snapshot();
        let found = self
            .tabs
            .iter()
            .position(|tab| match (playlist, &tab.playlist) {
                (Some(name), Some(open)) => open == name,
                (None, None) => tab.folder == folder,
                _ => false,
            });

        let Some(i) = found else { return false };
        if i != self.tab_idx {
            self.tab_idx = i;
            let tab = self.tabs[i].clone();
            self.restore(&tab);
        }
        self.focus = Pane::Tracks;
        true
    }

    /// `t`: opens whatever the sidebar cursor is on in a tab of its own, or
    /// takes you to the tab it is already in.
    pub fn open_in_new_tab(&mut self) {
        let already = match self.tab {
            Tab::Playlists => {
                let name = self
                    .playlists
                    .get(self.pl_cur)
                    .map(|(name, _)| name.clone());
                name.is_some_and(|name| self.jump_to_open(Some(&name), 0))
            }
            Tab::Folders => self.jump_to_open(None, self.folder_cur),
        };
        if already {
            self.info("already open");
            return;
        }

        let fresh = self.snapshot();
        self.tabs.insert(self.tab_idx + 1, fresh);
        self.tab_idx += 1;

        match self.tab {
            Tab::Playlists => self.open_playlist(),
            Tab::Folders => self.open_folder(),
        }
    }

    /// `:q` closes the tab. The last one closes vibox.
    pub fn close_tab(&mut self) {
        if self.tabs.len() < 2 {
            self.quit = true;
            return;
        }
        self.tabs.remove(self.tab_idx);
        self.tab_idx = self.tab_idx.min(self.tabs.len() - 1);
        let tab = self.tabs[self.tab_idx].clone();
        self.restore(&tab);
    }
}
