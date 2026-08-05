# vibox

**A jukebox you exit with `:q`.**

A cli music player with vi motions, ex commands, and tmux manners. Your library
is a directory, a track is a file, and the last line of the screen is the
command line.

```
* everything (61)            │      file                     artist          album              time
.                            │  1   Awolnation - Sail (Unli… AWOLNATION      RE/Sail            5:48
                             │    1 Band Of Horses - The Fu… Band of Horses  Acoustic at the …  5:05
                             │    2 Black Ops 2 Plaza Night… Skrillex feat.… [standalone reco…  3:51
                             │>   3 Boncabe                  Knife Party     Rage Valley        2:30
                             │    4 Cadillac by Panda Eyes…  Panda Eyes & A… Dubstep.NET Free…  3:51
                             │    5 Condukta - Open Circuit                  Dubstep            3:29
   0:47 ━━━━━━━━━━━━━━━━━────────────────────────────────────────────────────────────────  2:30
 NORMAL  Knife Party - Bonfire                                  vol 80%  path rep:-      4/61
 /bonf
```

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
| `gp` | jump to whatever is playing |
| `tab` `ctrl-w h/l` | switch pane |
| `:help` | every key, in sections |

Nothing needs a leader key and nothing needs a `ctrl-b` first, so it keeps out
of tmux's way.

## Commands

`:e ~/Music` opens a library or an m3u, `:sort artist`, `:vol 70`, `:seek 1:30`,
`:reload`, `:42` jumps to row 42, `:q`.

`:set` works the way vim's does: `:set noartist` hides a column, `:set artist!`
flips it, `:set lyrics` opens the lyrics pane, and `:set` on its own lists them.
Volume, shuffle and repeat come back the way you left them; `:mkrc` writes your
options to `~/.config/vibox/viboxrc` for the ones you want pinned.

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
