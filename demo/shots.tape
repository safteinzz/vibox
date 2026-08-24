# Stills, rendered by VHS instead of taken by hand. The README carries the gif
# alone - a picture of what the tour already shows is a picture nobody needs -
# so these are for anywhere else a still is wanted: an issue, a post, a mirror.
# No tmux, no window sizing, no cropping: the size below is the size of every
# shot, and a rerun after a ui change replaces them all identically.
#
#   cd demo && ./stage.sh up && vhs shots.tape && vhs changes.tape
#
# The files land straight in readme-assets/. Everything on screen comes from
# stage.sh: invented bands, silent audio, seeded lyrics, so a render never
# touches your music and never calls lrclib. `:changes` gets its own tape
# because that window fills the terminal, and a short one is the only way to
# frame it without cropping.

# VHS always records a video; with every frame hidden there is nothing to encode
# and it stops with "no frames". This one is a throwaway.
Output "/tmp/vibox-shots-throwaway.gif"

Set Shell bash
Set FontFamily "JetBrainsMono NF"
Set FontSize 15
# 1400x620 is 133 columns by 28 rows: the sidebar, every track column and the
# lyrics pane side by side, with the library filling the pane.
Set Width 1400
Set Height 620
Set Padding 20

Hide
Type "./stage.sh shell"
Enter
Sleep 2s
Type "clear"
Enter
Sleep 500ms
Type "vibox"
Enter
Sleep 2s
# Row 10 is `01 Night Ferry`, one of the two tracks the rig gives lyrics to.
Type ":10"
Enter
Sleep 500ms
Enter
Sleep 1s
Type "30l"
Sleep 300ms
Down
Down
Down
Sleep 1s
Show
Sleep 700ms

# ---- the main view -------------------------------------------------------
Screenshot ../readme-assets/browse.png
Sleep 700ms

# ---- lyrics --------------------------------------------------------------
Hide
Type ":set lyrics"
Enter
Sleep 2s
Show
Sleep 700ms
Screenshot ../readme-assets/lyrics.png
Sleep 700ms

# ---- the rename buffer ---------------------------------------------------
# Esc clears the message row, so the `:set` reply does not follow the shot into
# the next one.
Hide
Type ":set nolyrics"
Enter
Sleep 300ms
Escape
Sleep 300ms
# Row 19 is the first of the four files a ripper named, still sitting in the
# library next to everything else: that is what renaming in place looks like.
Type ":19"
Enter
Sleep 500ms
Type "c"
Sleep 300ms
Type "dwdwdwdwdw"
Sleep 200ms
Type "wwh"
Type "D"
Sleep 300ms
Type "j"
Sleep 200ms
Type "S"
Type "Slow Parade"
Escape
Sleep 200ms
Type "j"
Sleep 200ms
Type "S"
Type "Copper Wire"
Sleep 500ms
Show
Sleep 700ms
Screenshot ../readme-assets/rename.png
Sleep 700ms

# ---- playlists -----------------------------------------------------------
Hide
Escape
Sleep 200ms
Escape
Sleep 200ms
Type ":e!"
Enter
Sleep 500ms
Escape
Sleep 300ms
Tab
Type "gt"
Sleep 500ms
# `t` opens it in a tab of its own, so the tab bar shows both.
Type "t"
Sleep 1s
Show
Sleep 700ms
Screenshot ../readme-assets/playlists.png
Sleep 700ms

# ---- one more thing ------------------------------------------------------
# Back to the whole library with the lyrics pane up, because the rain is worth
# more over a full screen than over four rows.
Hide
Type "gT"
Sleep 500ms
Type ":set lyrics"
Enter
Sleep 300ms
Escape
Sleep 300ms
Type ":matrix"
Enter
Sleep 12s
Show
Sleep 700ms
Screenshot ../readme-assets/matrix.png
Sleep 1s
