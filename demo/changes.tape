# The `:changes` still, on its own because that window fills the terminal: the
# only way to frame a six line diff without cropping is a terminal six lines
# tall. Everything else is in shots.tape, and neither is in the README.
#
#   cd demo && vhs changes.tape
#
# It builds a batch worth reading: the four names from the rename scene, then a
# move and a delete, which need `:set danger`. Nothing is written - the tape
# never types `:w`, and the stage is thrown away by `./stage.sh down`.

Output "/tmp/vibox-changes-throwaway.gif"

Set Shell bash
Set FontFamily "JetBrainsMono NF"
Set FontSize 15
Set Width 1400
Set Height 250
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

# ---- four names ----------------------------------------------------------
Tab
Type "G"
Enter
Sleep 500ms
Type "c"
Sleep 200ms
Type "dwdwdwdwdw"
Type "wwh"
Type "D"
Type "j"
Type "S"
Type "Slow Parade"
Escape
Type "j"
Type "S"
Type "Copper Wire"
Escape
Type "j"
Type "S"
Type "Tin Roof"
Escape
Sleep 300ms
Escape
Sleep 300ms

# ---- a move and a delete -------------------------------------------------
Type ":set danger"
Enter
Sleep 300ms
Escape
Sleep 300ms
# Junco Wire/Night Ferry, three rows up the sidebar, and cut a track out of it.
Tab
Type "kkk"
Enter
Sleep 300ms
Type "jj"
Type "dd"
Sleep 300ms
# Put it down in another folder: that is what makes it a move rather than a
# delete.
Tab
Type "jj"
Enter
Sleep 300ms
Type "p"
Sleep 500ms
# ...and one cut that never gets put back, which is what a delete is here.
Type "gg"
Type "j"
Type "dd"
Sleep 500ms

Type ":changes"
Enter
Sleep 1s
Show
Sleep 700ms
Screenshot ../readme-assets/changes.png
Sleep 1s
