#!/usr/bin/env bash
# A staged music library for the README screenshots and the demo GIF: invented
# artists, invented albums, invented lyrics, silent audio generated on the spot.
# Nothing here touches your real music, your real config or your real state -
# every path is redirected into ./home, XDG variables included.
#
#   ./stage.sh up     build the library and the fixtures
#   ./stage.sh run    launch vibox against them
#   ./stage.sh shell  a shell where `vibox` is this build, for the tapes
#   ./stage.sh down   delete the stage
#
# Every band, album, track and lyric below is made up, and every audio file is
# the same two-note drone encoded by ffmpeg with the tags written in. There is
# nothing real in it to leak, and no track anyone has rights to.
#
# It is a drone and not silence because `:matrix` falls to how loud the audio
# going out right now is, and silence makes it a blank screen. So a render makes
# a noise: turn the speakers down before you start one.
#
# The lyric cache is seeded too, so `:set lyrics` shows words with no network
# call: the two tracks with lyrics get invented ones, and every other track gets
# an empty entry, which is what vibox writes for "lrclib has nothing". A render
# therefore never asks lrclib about a band that does not exist.
#
# Needs ffmpeg on PATH, for the fixtures only. Nothing vibox itself ships uses it.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STAGE="$HERE/home"
MUSIC="$STAGE/Music"
VIBOX="$HERE/../target/release/vibox"

# HOME alone is not enough: a shell that exports XDG_DATA_HOME would send the
# playlists, the lyric cache and the session state back to the real one.
# PULSE_COOKIE points back at the real home on purpose - the sound server is the
# one thing that is not staged, and without its cookie there is no playback to
# film. XDG_RUNTIME_DIR carries through untouched for the same reason.
stage_env() {
  ENV_ARGS=(
    "HOME=$STAGE"
    "XDG_CONFIG_HOME=$STAGE/.config"
    "XDG_DATA_HOME=$STAGE/.local/share"
    "XDG_STATE_HOME=$STAGE/.local/state"
    "XDG_CACHE_HOME=$STAGE/.cache"
    "PULSE_COOKIE=$HOME/.config/pulse/cookie"
  )
}

# ---- the library ---------------------------------------------------------
# `<seconds>|<track>|<title>|<filename>`, one line per file, read by make_album.
# The filenames in Unsorted are the point of the rename scenes: the tags are
# right and the names are what a ripper left behind.

album_halcyon_slow_static=$(cat <<'ROWS'
222|1|Reservoir|01 Reservoir
256|2|Slow Static|02 Slow Static
178|3|Anemone|03 Anemone
301|4|Paper Wing|04 Paper Wing
207|5|Undertow|05 Undertow
ROWS
)

album_halcyon_marram=$(cat <<'ROWS'
248|1|Marram|01 Marram
199|2|Sandbar|02 Sandbar
372|3|Low Tide Radio|03 Low Tide Radio
224|4|Ferrous|04 Ferrous
ROWS
)

album_junco_night_ferry=$(cat <<'ROWS'
154|1|Night Ferry|01 Night Ferry
191|2|Kestrel|02 Kestrel
242|3|Winter Count|03 Winter Count
218|4|Halfway House|04 Halfway House
167|5|Blue Amp|05 Blue Amp
ROWS
)

album_vane_paper_lantern=$(cat <<'ROWS'
236|1|Paper Lantern|01 Paper Lantern
261|2|Cinders|02 Cinders
314|3|Ostinato|03 Ostinato
108|4|Lantern Reprise|04 Lantern Reprise
ROWS
)

# Downloaded and never tidied. Same three bands, tags intact, names a mess.
unsorted=$(cat <<'ROWS'
203|1|Salt Flats|Marisol Vane|Salt Flats|01. Marisol Vane -- Salt Flats (Official Audio) [4K]
187|1|Copper Wire|Halcyon Bus|Copper Wire|Halcyon_Bus_-_Copper_Wire_LYRICS_HQ_128kbps
229|1|Tin Roof|Junco Wire|Tin Roof|y2mate.com - Junco Wire Tin Roof Live Session
174|1|Slow Parade|Marisol Vane|Slow Parade|03 - marisol vane - slow parade (320kbps)
ROWS
)

# Two detuned sines under a slow tremolo, once, as long as the longest track.
# Every file is a prefix of this, so the whole library costs one synthesis and
# 22 cheap encodes.
MASTER=""
make_master() {
  MASTER="$(mktemp -t vibox-demo-XXXXXX.wav)"
  ffmpeg -nostdin -hide_banner -loglevel error -y -f lavfi \
    -i "aevalsrc=0.20*sin(2*PI*196*t)*(0.55+0.45*sin(2*PI*0.3*t))+0.13*sin(2*PI*294*t)*(0.55+0.45*sin(2*PI*0.17*t)):s=44100:d=400" \
    -c:a pcm_s16le "$MASTER"
}

# One tagged file, cut out of the master. The duration is what the track list
# and the progress bar read.
make_track() {
  local path="$1" secs="$2" title="$3" artist="$4" album="$5" track="$6" codec="$7"
  # -nostdin, or ffmpeg eats the rows the `while read` loop above is still
  # reading and every other track comes out with its first digit missing.
  local args=(-nostdin -hide_banner -loglevel error -y
    -i "$MASTER" -t "$secs"
    -metadata "title=$title" -metadata "artist=$artist"
    -metadata "album=$album" -metadata "track=$track"
    -metadata "date=2024")
  if [ "$codec" = flac ]; then
    args+=(-c:a flac -compression_level 5)
  else
    args+=(-c:a libmp3lame -b:a 64k)
  fi
  ffmpeg "${args[@]}" "$path"
}

make_album() {
  local artist="$1" album="$2" codec="$3" rows="$4"
  local dir="$MUSIC/$artist/$album"
  mkdir -p "$dir"
  local secs no title file
  while IFS='|' read -r secs no title file; do
    [ -n "$secs" ] || continue
    make_track "$dir/$file.$codec" "$secs" "$title" "$artist" "$album" "$no" "$codec"
  done <<< "$rows"
}

make_unsorted() {
  local dir="$MUSIC/Unsorted"
  mkdir -p "$dir"
  local secs no title artist album file
  while IFS='|' read -r secs no title artist album file; do
    [ -n "$secs" ] || continue
    make_track "$dir/$file.mp3" "$secs" "$title" "$artist" "$album" "$no" mp3
  done <<< "$unsorted"
}

# ---- the lyric cache -----------------------------------------------------
# vibox names a cache entry after a hash of the track's absolute path, so the
# name has to be computed the same way here: acc = acc * 31 + byte, wrapping at
# 64 bits, printed as 16 hex digits. Bash arithmetic wraps the same way, and
# every path in this rig is ascii, so a character is a byte.
cache_name() {
  local path="$1" acc=0 i b
  for ((i = 0; i < ${#path}; i++)); do
    printf -v b '%d' "'${path:i:1}"
    acc=$((acc * 31 + b))
  done
  printf '%016x.lrc' "$acc"
}

# `[vibox:3]` is CACHE_MARK in src/lyrics.rs. An entry without the current mark
# is refetched rather than trusted, so this line moves when that one does.
CACHE_MARK='[vibox:3]'

seed_lyrics() {
  local dir="$STAGE/.local/share/vibox/lyrics"
  mkdir -p "$dir"

  # Every track gets an entry, so nothing in this library is ever looked up.
  # An empty body is what vibox writes when lrclib has nothing to give.
  local f
  while IFS= read -r f; do
    printf '%s\n' "$CACHE_MARK" > "$dir/$(cache_name "$f")"
  done < <(find "$MUSIC" -type f \( -name '*.mp3' -o -name '*.flac' \) | sort)

  # ...and the two the tapes play get words. Invented, like the band.
  cat > "$dir/$(cache_name "$MUSIC/Junco Wire/Night Ferry/01 Night Ferry.mp3")" <<LRC
$CACHE_MARK
[00:04.00] the last boat leaves at ten to one
[00:11.50] and everybody knows the way
[00:19.00] the harbour lights go one by one
[00:26.50] the water keeps whatever fell
[00:34.00] i counted every rope and rail
[00:41.50] and still i could not tell you when
[00:49.00] the night ferry stopped being a boat
[00:56.50] and started being somewhere else
[01:04.00] so hold the rail and watch the town
[01:11.50] go quiet as a folded coat
[01:19.00] there is nothing here to bring you down
[01:26.50] there is only water and the boat
[01:34.00] the last boat leaves at ten to one
[01:41.50] and everybody knows the way
[01:49.00] and nobody is coming back
LRC

  cat > "$dir/$(cache_name "$MUSIC/Junco Wire/Night Ferry/02 Kestrel.mp3")" <<LRC
$CACHE_MARK
[00:05.00] a kestrel on the wire again
[00:13.00] holding still against the wind
[00:21.00] the whole field waiting underneath
[00:29.00] for something small to move
[00:37.00] i have been holding still for years
[00:45.00] and nothing small has moved
LRC
}

# ---- playlists, config, state -------------------------------------------
# Extended m3u, absolute paths: the format vibox writes, so a staged playlist is
# indistinguishable from one it made itself.
seed_playlists() {
  local dir="$STAGE/.local/share/vibox/playlists"
  mkdir -p "$dir"
  {
    echo "#EXTM3U"
    m3u_line 154 "Junco Wire" "Night Ferry" "$MUSIC/Junco Wire/Night Ferry/01 Night Ferry.mp3"
    m3u_line 222 "Halcyon Bus" "Reservoir" "$MUSIC/Halcyon Bus/Slow Static/01 Reservoir.flac"
    m3u_line 108 "Marisol Vane" "Lantern Reprise" "$MUSIC/Marisol Vane/Paper Lantern/04 Lantern Reprise.mp3"
    m3u_line 372 "Halcyon Bus" "Low Tide Radio" "$MUSIC/Halcyon Bus/Marram/03 Low Tide Radio.flac"
    m3u_line 167 "Junco Wire" "Blue Amp" "$MUSIC/Junco Wire/Night Ferry/05 Blue Amp.mp3"
    m3u_line 314 "Marisol Vane" "Ostinato" "$MUSIC/Marisol Vane/Paper Lantern/03 Ostinato.mp3"
    m3u_line 178 "Halcyon Bus" "Anemone" "$MUSIC/Halcyon Bus/Slow Static/03 Anemone.flac"
    m3u_line 218 "Junco Wire" "Halfway House" "$MUSIC/Junco Wire/Night Ferry/04 Halfway House.mp3"
    m3u_line 199 "Halcyon Bus" "Sandbar" "$MUSIC/Halcyon Bus/Marram/02 Sandbar.flac"
  } > "$dir/late shift.m3u"
  {
    echo "#EXTM3U"
    m3u_line 178 "Halcyon Bus" "Anemone" "$MUSIC/Halcyon Bus/Slow Static/03 Anemone.flac"
    m3u_line 236 "Marisol Vane" "Paper Lantern" "$MUSIC/Marisol Vane/Paper Lantern/01 Paper Lantern.mp3"
  } > "$dir/rainy sunday.m3u"
}

m3u_line() {
  printf '#EXTINF:%s,%s - %s\n%s\n' "$1" "$2" "$3" "$4"
}

# The rc file is ex commands, so `vibox` with no argument opens the staged
# library and the frame shows the command a user would really type.
seed_config() {
  mkdir -p "$STAGE/.config/vibox"
  cat > "$STAGE/.config/vibox/viboxrc" <<'RC'
" ~/.config/vibox/viboxrc - ex commands, run at startup
set root=~/Music
vol 65
RC
}

up() {
  need ffmpeg
  need "$VIBOX"
  down_quiet
  mkdir -p "$MUSIC"
  echo "encoding fixtures..."
  make_master
  make_album "Halcyon Bus"  "Slow Static"   flac "$album_halcyon_slow_static"
  make_album "Halcyon Bus"  "Marram"        flac "$album_halcyon_marram"
  make_album "Junco Wire"   "Night Ferry"   mp3  "$album_junco_night_ferry"
  make_album "Marisol Vane" "Paper Lantern" mp3  "$album_vane_paper_lantern"
  make_unsorted
  seed_lyrics
  seed_playlists
  seed_config
  rm -f "$MASTER"
  echo "staged in $STAGE"
  echo
  echo "  ./stage.sh run    open vibox against it"
  echo "  ./stage.sh shell  a shell where \`vibox\` is this build"
  echo "  ./stage.sh down   delete it"
}

need() {
  command -v "$1" > /dev/null 2>&1 || [ -x "$1" ] || {
    echo "missing \`$1\`" >&2
    exit 1
  }
}

# Nothing here mounts anything, but a recursive delete in a convenience script
# is worth a guard anyway: refuse if something has appeared under the stage, and
# keep --one-file-system as a second net.
down_quiet() {
  [ -d "$STAGE" ] || return 0
  if awk -v s="$STAGE/" '$2 ~ "^"s {found=1} END {exit !found}' /proc/mounts; then
    echo "REFUSING to delete $STAGE: something is mounted under it." >&2
    exit 1
  fi
  rm -rf --one-file-system "$STAGE"
}

# A shell that finds this build as `vibox`, so the frame shows the command you
# would type rather than a path into target/release. It keeps your own prompt:
# this runs before HOME is redirected, so $HOME here is still the real one, and
# a staged prompt looks staged.
open_shell() {
  stage_env
  mkdir -p "$HERE/bin"
  ln -sf "$(cd "$(dirname "$VIBOX")" && pwd)/vibox" "$HERE/bin/vibox"
  {
    echo "[ -f '$HOME/.bashrc' ] && . '$HOME/.bashrc'"
    echo "clear"
  } > "$HERE/shellrc"
  (cd "$STAGE" && env "${ENV_ARGS[@]}" \
    PATH="$HERE/bin:$PATH" \
    STARSHIP_CONFIG="$HOME/.config/starship.toml" \
    bash --noprofile --rcfile "$HERE/shellrc" -i)
}

case "${1:-up}" in
  up) up ;;
  # From inside the staged home, so `~` on screen is the fixture and never you.
  run)
    stage_env
    (cd "$STAGE" && env "${ENV_ARGS[@]}" "$VIBOX")
    ;;
  shell) open_shell ;;
  down)
    down_quiet
    echo "torn down"
    ;;
  *)
    echo "usage: $0 [up|run|shell|down]" >&2
    exit 2
    ;;
esac
