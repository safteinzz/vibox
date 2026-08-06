# vibox

> **Canonical:** [gitlab.com/safteinzz/vibox](https://gitlab.com/safteinzz/vibox) · **Mirror:** [github.com/safteinzz/vibox](https://github.com/safteinzz/vibox)

**A jukebox you exit with `:q`**

A cli music player with vi motions, ex commands, and tmux manners. Your library
is a directory, a track is a file, and the last line of the screen is the
command line.

**vibox does not touch your files.** It plays them, organizes them into
playlists, and renames them when you ask. If you want it to move, copy and
delete as well, that is `:set danger`, off by default and never saved for you.
Either way nothing reaches the disk until you write it: `:changes` shows what a
`:w` would do, `u` takes it back, and a force quit loses the lot on purpose.

```
 folders | playlists         │ everything │ roadtrip* │
friday                       │      file                       artist           album              time
late night                   │    4 Daft Punk - Around the Wo… Daft Punk        Homework           7:09
roadtrip                     │    3 Fleetwood Mac - Dreams     Fleetwood Mac    Rumours            4:17
                             │    2 Massive Attack - Teardrop  Massive Attack   Mezzanine          5:29
                             │    1 Nirvana - Smells Like Tee… Nirvana          Nevermind          5:01
                             │  5   Pink Floyd - Comfortably … Pink Floyd       The Wall           6:23
                             │>   1 Rick Astley - Never Gonna… Rick Astley      Whenever You Nee…  3:34
                             │    2 The Prodigy - Breathe      The Prodigy      The Fat of the L…  5:35
   1:58 ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━───────────────────────────────────────   3:34
 NORMAL  Rick Astley - Never Gonna Give You Up                         vol 80%  artist rep:- shf  5/15
put 2 tracks into `roadtrip`, `:w` saves
```

Folders or playlists on the left, the open views as tabs across the top, the
playing track marked with `>`, and relative line numbers in the gutter so `4j`
lands where you counted. The `*` on a tab means it has changes that `:w` has not
written yet.

```bash
cargo install vibox   # that is the whole install: no system libraries, no headers
vibox ~/Music         # or just `vibox`, for your music directory
vibox rotation.m3u    # a playlist is a library too
```

Every dependency is pure rust, so the install cannot fail on a missing C header.
Sound goes out over the pulseaudio socket, which pipewire serves as well. Linux
only for now.

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
| `/` `?` `n` `N` | search filenames, artists, albums |
| `c` | edit filenames in place, `:w` renames them |
| `gt` `gT` `t` | switch tabs in the focused pane, open one in a new tab |
| `gp` | jump to whatever is playing |
| `tab` `ctrl-w h/l` | switch pane |
| `:help` | every key, in sections |

Nothing needs a leader key and nothing needs a `ctrl-b` first, so it keeps out
of tmux's way.

## Commands

`:set root=~/Music` is the library vibox opens on its own; `vibox ~/other` wins
for that run. `:e <path>` opens one for now, `:sort artist`, `:vol 70`,
`:seek 1:30`, `:reload`, `:42` jumps to row 42, `:q` closes the tab.

`:set` works the way vim's does: `:set noartist` hides a column, `:set artist!`
flips it, `:set lyrics` opens the lyrics pane, and `:set` on its own lists them.
Volume, shuffle and repeat come back the way you left them; `:mkrc` writes your
options to `~/.config/vibox/viboxrc` for the ones you want pinned.

## Playlists

Playlists are m3u files in `~/.local/share/vibox/playlists`, and the left pane
switches to them with `gt`. `o` starts an empty one, `enter` shows one in the
track list, `t` opens it in a tab of its own.

Filling one is a yank and a put: `t` a folder into its own tab, `V` and `j` to
select, `y`, `gt` back to the playlist, `p`. `dd` cuts a track out and `p` puts
it back where you want it, so that is also how you reorder one. `:w` saves. A playlist is a view over your library, so
the folders tab keeps browsing everything while one is open.

Nothing is written until `:w`. A `[+]` on the statusline means something is
waiting, `:changes` lists exactly what `:w` would do, and `:q` refuses until you
either write it or throw it away with `:q!`.

## Danger mode

`:set danger` lets vibox change your library, and nothing else does. With it on,
`dd` cuts tracks or a whole folder, `d` then `p` puts them in another folder as
a move, `y` then `p` copies them, and `:mkdir jazz` makes a folder. It is off
every time vibox starts, is never written by `:mkrc`, and has to be typed into
`~/.config/vibox/viboxrc` by hand to be there for you.

A cut is an edit, not an action: the rows leave the list at once, `:changes`
lists every one, `u` and `ctrl-r` walk them, and only `:w` writes. Cut something
and never put it back and `:w` deletes it, which is the whole point of `:changes`
being there to read first. A batch is all or nothing, so one name that already
exists stops the entire write rather than half applying it.

## Renaming

`c` turns the list into a buffer of filenames. Edit them where they sit with the
operators you already use (`cw`, `x`, `A`, `dw`), move between rows with `j` and
`k`, then `:w` renames every changed file at once, or `:e!` throws the edits
away. Nothing touches disk until you write, every name is checked before any
rename runs, and tags are never modified.

## Lyrics

`:set lyrics` fetches from lrclib and follows the song, highlighting the line
being sung. It only follows when lrclib's recording matches yours within a
couple of seconds, because timings from a different edit would drift; otherwise
you get the words with no highlight. Off by default, cached on disk.

## Media keys

vibox is an MPRIS player, so the media keys on your keyboard reach it from
anywhere, and so does `playerctl -p vibox play-pause`.

## License

AGPL-3.0-only.
