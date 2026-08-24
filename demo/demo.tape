# The tour at the top of the README, rendered by VHS. Nothing here is recorded
# off a screen: VHS replays these keystrokes in a headless terminal and draws
# the gif itself, so it re-renders identically every release.
#
#   cd demo && ./stage.sh up && vhs demo.tape
#
# One beat per thing vibox does, in the order you would meet them, and each beat
# finishes before the next starts. It writes for real (`:w` renames four files)
# and it plays for real, so run `./stage.sh up` again before another take.

Output "../readme-assets/demo.gif"

Set Shell bash
Set FontFamily "JetBrainsMono NF"
Set FontSize 15
Set Width 1400
Set Height 620
Set Padding 20
Set Framerate 20
Set TypingSpeed 55ms

Hide
Type "./stage.sh shell"
Enter
Sleep 2s
Type "clear"
Enter
Sleep 500ms
Show

# ===========================================================================
# 1 - a directory of music, and a jukebox on top of it
# ===========================================================================
Type "vibox"
Sleep 300ms
Enter
Sleep 2s

# ===========================================================================
# 2 - vi motions, and enter plays
# ===========================================================================
# The gutter is relative, so the count you can read is the count you type.
# Nine rows down is `01 Night Ferry`, the one the rig wrote lyrics for.
Type "5j"
Sleep 800ms
Type "4j"
Sleep 800ms
Enter
Sleep 2.5s

# ===========================================================================
# 3 - `/` searches the columns you can see
# ===========================================================================
Type "/lantern"
Sleep 700ms
Enter
Sleep 1.2s
Type "n"
Sleep 1.2s

# ===========================================================================
# 4 - the list is a buffer of filenames
# ===========================================================================
# Four files a ripper named, at the bottom of the library. `c` opens them where
# they sit; `S` clears a name, and the last one gets the vi treatment instead:
# five `dw` to eat the prefix, `D` to cut the tail off.
Type "G"
Sleep 800ms
Type "c"
Sleep 800ms
Type "S"
Type "Tin Roof"
Escape
Sleep 400ms
Type "k"
Sleep 400ms
Type "S"
Type "Copper Wire"
Escape
Sleep 400ms
Type "k"
Sleep 400ms
Type "S"
Type "Slow Parade"
Escape
Sleep 400ms
Type "k"
Sleep 600ms
# `0` first: moving between rows keeps the column, and these five `dw` have to
# start at the front of the name.
Type "0"
Sleep 300ms
Type "dw"
Sleep 250ms
Type "dw"
Sleep 250ms
Type "dw"
Sleep 250ms
Type "dw"
Sleep 250ms
Type "dw"
Sleep 500ms
Type "ww"
Sleep 300ms
Type "h"
Type "D"
Sleep 1.5s
Escape
Sleep 1s

# ===========================================================================
# 5 - nothing has touched the disk yet
# ===========================================================================
Type ":changes"
Sleep 400ms
Enter
Sleep 3.5s
Type "q"
Sleep 800ms

# ===========================================================================
# 6 - and now it has
# ===========================================================================
Type ":w"
Sleep 400ms
Enter
Sleep 3s

# ===========================================================================
# 7 - words, in time with the music
# ===========================================================================
Type ":set lyrics"
Sleep 400ms
Enter
Sleep 2.5s
# 30l is thirty seconds forward: counts work on every key that takes one.
Type "30l"
Sleep 3.5s

# ===========================================================================
# 8 - playlists are m3u files, and each opens in its own tab
# ===========================================================================
Tab
Sleep 500ms
Type "gt"
Sleep 800ms
Type "t"
Sleep 2.5s
Type "gT"
Sleep 1.5s

# ===========================================================================
# 9 - one more thing
# ===========================================================================
Type ":matrix"
Sleep 400ms
Enter
Sleep 3.5s

# ===========================================================================
# 10 - a jukebox you exit with :q
# ===========================================================================
Type ":q"
Sleep 400ms
Enter
Sleep 1.5s
