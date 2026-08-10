# vibox

> **Canonical:** [gitlab.com/safteinzz/vibox](https://gitlab.com/safteinzz/vibox) · **Mirror:** [github.com/safteinzz/vibox](https://github.com/safteinzz/vibox)

**A jukebox you exit with `:q`**

A cli music player with vi motions, ex commands, and tmux manners. Your library
is a directory, a track is a file, and the last line of the screen is the
command line. There is no database and no import step: point it at a folder and
it plays.

**vibox does not touch your files.** It plays them, organizes them into
playlists, and renames them when you ask. If you want it to move, copy and
delete as well, that is `:set danger`, off by default and never saved for you.
Either way nothing reaches the disk until you write it: `:changes` shows what a
`:w` would do, `u` takes it back, and a force quit loses the lot on purpose.

## Install

```bash
cargo install vibox   # that is the whole install: no system libraries, no headers
vibox ~/Music         # or just `vibox`, for your music directory
vibox rotation.m3u    # a playlist is a library too
```

Every dependency is pure rust, so the install cannot fail on a missing C header.
Sound goes out over the pulseaudio socket, which pipewire serves as well. Linux
only for now.

## Browse

Folders or playlists on the left, open views as tabs across the top, `>` on the
playing track, and relative line numbers in the gutter so `4j` lands where you
counted.

![The vibox interface: a folder list on the left, a track list with artist and duration columns on the right, the playing track highlighted, and a progress bar above the status line](https://gitlab.com/safteinzz/vibox/-/raw/main/readme-assets/browse.png)

Each tab keeps its own cursor, scroll and sort, so searching in one leaves the
others where you left them. `/` filters on what you can see: filenames, artists
and albums, never the path.

## Rename files in place

`c` turns the list into a buffer of filenames. Edit them where they sit with the
operators you already use (`cw`, `x`, `A`, `dw`) and move between rows with `j`
and `k`.

![The track list in edit mode with two filenames changed to new names, each marked with a tilde in the gutter, the cursor sitting inside a third name, and EDIT and a plus marker on the status line](https://gitlab.com/safteinzz/vibox/-/raw/main/readme-assets/rename.png)

`~` marks a row you changed and `[+]` means something is waiting to be written.
Every name is checked before any rename runs, and tags are never modified.

## Nothing is written until `:w`

![A popup headed `:w would do this` listing two renames from old filename to new and two deletions in red, over the track list](https://gitlab.com/safteinzz/vibox/-/raw/main/readme-assets/changes.png)

Renames, playlist edits, cuts and deletions all sit in memory until you write
them. `:changes` lists exactly what `:w` would do, `u` and `ctrl-r` walk them,
`:e!` throws the lot away, and `:q` refuses while anything is pending. A batch is
all or nothing, so one name that already exists stops the whole write rather
than half applying it.

## Playlists

Playlists are m3u files in `~/.local/share/vibox/playlists`, and the left pane
switches to them with `gt`. `o` starts an empty one, `enter` shows one in the
track list, `t` opens it in a tab of its own.

Filling one is a yank and a put: `t` a folder into its own tab, `V` and `j` to
select, `y`, `gt` back to the playlist, `p`. `dd` cuts a track out and `p` puts
it back where you want it, so that is also how you reorder one. A playlist is a
view over your library, so the folders tab keeps browsing everything while one is
open.

## Lyrics

`:set lyrics` fetches from lrclib and follows the song, highlighting the line
being sung.

![The interface with a third pane on the right showing song lyrics, the line currently being sung highlighted in bold](https://gitlab.com/safteinzz/vibox/-/raw/main/readme-assets/lyrics.png)

It only follows when lrclib's recording matches yours within a couple of seconds,
because timings from a different edit would drift; otherwise you get the words
with no highlight. Off by default, cached on disk.

## Danger mode

`:set danger` lets vibox change your library, and nothing else does. With it on,
`dd` cuts tracks or a whole folder, `d` then `p` puts them in another folder as
a move, `y` then `p` copies them, and `:mkdir jazz` makes a folder. It is off
every time vibox starts, is never written by `:mkrc`, and has to be typed into
`~/.config/vibox/viboxrc` by hand to be there for you.

A cut is an edit, not an action: the rows leave the list at once, `:changes`
lists every one, and only `:w` writes. Cut something and never put it back and
`:w` deletes it, which is the whole point of `:changes` being there to read
first.

## Keys

Motions are vi motions: `j k`, `gg`, `G`, `12G`, `ctrl-d`, `ctrl-u`, `H M L`,
`zz zt zb`. Counts work everywhere, so `8j` moves eight rows and `30l` seeks
thirty seconds.

| key | does |
| --- | --- |
| `enter` `space` | play the track under the cursor, pause |
| `h` `l` | seek 5s back or forward |
| `<` `>` `+` `-` `m` | previous, next, volume, mute |
| `r` `s` | repeat off/all/one, shuffle |
| `c` | edit filenames in place, `:w` renames them |
| `/` `?` `n` `N` | search filenames, artists, albums |
| `*` `#` | next, previous track by the artist under the cursor |
| `gt` `gT` `t` | switch tabs in the focused pane, open one in a new tab |
| `gp` `K` | jump to whatever is playing, show track info |
| `tab` `ctrl-w h/l` | switch pane |
| `:help` | every key, in sections |

Nothing needs a leader key and nothing needs a `ctrl-b` first, so it keeps out
of tmux's way.

## Commands

```
:e <path>       open a directory or an m3u for this session
:set root=~/Music   the library vibox opens on its own
:set lyrics     lyrics pane; :set noartist hides a column, :set artist! flips it
:set danger     let vibox move, copy and delete; off every start, never saved
:sort artist    path, title, artist, album, duration
:vol 70         :seek 1:30, :reload, :42 jumps to row 42
:changes        what :w would do        :w writes it, :e! discards it
:mkrc           save your options to ~/.config/vibox/viboxrc
:q  :q!         close the tab, or leave without writing
```

`:set` works the way vim's does, and `:set` on its own lists everything. Volume,
shuffle and repeat come back the way you left them.

## Media keys

vibox is an MPRIS player, so the media keys on your keyboard reach it from
anywhere, and so does `playerctl -p vibox play-pause`.

## One more thing

![The whole interface under a rain of falling green characters, the track list and lyrics still legible underneath](https://gitlab.com/safteinzz/vibox/-/raw/main/readme-assets/matrix.png)

There is one command for this. It is not in the table above, it is not in
`:help`, and you are not getting it from me.

## License

AGPL-3.0-only.
