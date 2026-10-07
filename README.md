# vibox

> **Canonical:** [gitlab.com/safteinzz/vibox](https://gitlab.com/safteinzz/vibox) · **Mirror:** [github.com/safteinzz/vibox](https://github.com/safteinzz/vibox)

<!-- desc:start -->
a jukebox you exit with :q - a cli music player whose library is a vim buffer, nothing written until :w
<!-- desc:end -->

## Install

```bash
cargo install vibox
vibox self check   # is a newer release out?
vibox self update  # install the latest
```

No cargo yet? Rust installs the same way on every distro: [rustup.rs](https://rustup.rs).

![A tour of vibox: playing a track with vi motions, searching, renaming four badly named files in place, reading the pending diff with :changes, writing it with :w, the lyrics pane following the song, and a playlist opening in its own tab](https://gitlab.com/safteinzz/vibox/-/raw/main/readme-assets/demo.gif)

## Open a library

```bash
vibox                 # your music directory
vibox ~/Music         # or any folder
vibox rotation.m3u    # a playlist is a library too
```

## Rename files in place

`c` turns the list into a buffer of filenames, edited where they sit with the
operators you already use (`cw`, `x`, `A`, `dw`) and `j` and `k` between rows.
`~` marks a row you changed, `[+]` means something is waiting, and tags are
never modified.

## Nothing is written until `:w`

Renames, moves, copies and playlist edits sit in memory until you write them,
and `:changes` shows what `:w` would do. The batch is checked against the disk
first, so a clash stops the whole write and nothing is ever half applied.

## Changing the library itself

It is vim, and the folder is the buffer: `dd` a track, `t` its new folder into
a tab, `gt` over and `p`, or `yy` to copy it instead. The move is an edit like
any other, so `u` takes it back and only `:w` writes it.

## Playlists

Filling a playlist is a yank and a put: `t` a folder into its own tab, `V` and
`j` to select, `y`, `gt` to the playlist, `p`. `dd` and `p` reorder it the same
way.

## Lyrics

`:set lyrics` fetches from lrclib and follows the song. When a sheet sits a few
seconds out, `[` and `]` shift it and the correction is kept. The pane takes the
keyboard like any other, so the usual motions scroll it. Off by default, cached
on disk.

## Commands

```
:e <path>       open a directory or an m3u for this session
:set root=~/Music   the library vibox opens on its own
:set lyrics     lyrics pane; :set noartist hides a column, :set artist! flips it
:sort artist    path, title, artist, album, duration
:vol 70         :seek 1:30, :reload, :42 jumps to row 42
:changes        what :w would do        :w writes it, :e! discards it
:history        every track played this session, in order, j and k scroll
:clearcache     drop every cached lyric so they are fetched again
:mkrc           save your options to ~/.config/vibox/viboxrc
:matrix         wake up
:q  :q!         close the tab, or leave without writing
```

`:set` works the way vim's does and lists everything on its own, and `:help`
lists every key.

vibox takes over the terminal and prints nothing for a pipe, but it is an MPRIS
player, so the media keys and `playerctl -p vibox play-pause` reach it from
anywhere.

## Where it keeps things

```
~/.config/vibox/viboxrc              ex commands run at startup, written by :mkrc
~/.local/share/vibox/state           volume, shuffle, repeat, columns, on quit
~/.local/share/vibox/playlists/      your m3u files
~/.local/share/vibox/lyrics/         the lyric cache, dropped by :clearcache
```

Nothing else is written until you type `:w`, so a crash or a kill loses only
what was pending.

## Compatibility

Linux. Audio goes out over the pulseaudio socket, which pipewire serves as well,
so nothing needs installing beyond the binary.

## License

AGPL-3.0-only
