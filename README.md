# vibox

**a jukebox you exit with `:q`.** 🎵

A cli music player with vi motions, ex commands, and tmux manners. Your library
is a directory, a playlist is an m3u, and the last line of the screen is the
command line.

```bash
cargo install vibox   # the whole install: no system libraries, no headers
vibox ~/Music         # or just `vibox`, for your music directory
vibox rotation.m3u    # a playlist is a library too
```

## Keys

`j k`, `gg`, `G`, `12G`, `ctrl-d`, `ctrl-u`, `H M L`, `zz zt zb`. Counts work:
`8j`, and `30l` seeks 30 seconds.

| key | does |
| --- | --- |
| `enter` `space` | play the track under the cursor, pause |
| `h` `l` | seek 5s back or forward |
| `<` `>` `+` `-` `m` | previous, next, volume, mute |
| `r` `s` | repeat off/all/one, shuffle |
| `/` `?` `n` `N` | search files, artists, albums |
| `c` | edit filenames in the list, `:w` renames them |
| `tab` `ctrl-w h/l` | switch pane |
| `:help` | all of them, in sections |

Nothing needs a leader key, so it stays out of tmux's way.

## Commands

`:e ~/Music` open a library or an m3u, `:sort artist`, `:vol 70`, `:seek 1:30`,
`:set noartist` hide a column, `:set lyrics` show lyrics, `:mkrc` keep your
options, `:reload`, `:42`, `:q`.

## Renaming

`c` turns the list into a buffer of filenames. Edit them where they sit with the
motions and operators you already use (`cw`, `x`, `A`, `dw`), move between rows
with `j` and `k`, then `:w` renames every changed file at once. `:e!` throws the
pending edits away. Nothing touches disk until you write, names are checked
before any rename happens, and tags are never modified.

## Media keys

vibox is an MPRIS player, so the media keys work from anywhere, and so does
`playerctl -p vibox play-pause`.

## License

AGPL-3.0-only.
