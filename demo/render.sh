#!/usr/bin/env bash
# Render the README asset in a container, so a machine needs podman or docker
# and nothing else: no vhs, no ffmpeg, no sound server, no font, and the same
# frames on every machine that runs it.
#
#   ./render.sh              every tape
#   ./render.sh demo         one tape
#
# Each tape runs in the image from ./Dockerfile, in a container of its own that
# stages, records and tears down, because `demo.tape` types `:w` and really
# renames the fixtures. vibox plays through pulseaudio, so the container starts
# one with a null sink before vhs: the fixtures are silence, but the progress
# bar and the lyrics only move while playback really runs.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

ENGINE="$(command -v podman || command -v docker || true)"
[ -n "$ENGINE" ] || { echo "render.sh needs podman or docker" >&2; exit 1; }
IMAGE=localhost/vibox-render

TAPES=("$@")
[ ${#TAPES[@]} -gt 0 ] || TAPES=(demo)

(cd .. && cargo build --release)
# The Dockerfile is the whole build context: nothing in this folder is copied in.
"$ENGINE" build -q -t "$IMAGE" - < Dockerfile > /dev/null

# Run as this user, not root: the images it writes stay yours, and the staged
# shell's prompt ends in `$` as it does on a machine rendering without a
# container, rather than root's `#`.
if [ "$(basename "$ENGINE")" = docker ]; then
  USER_ARGS=(--user "$(id -u):$(id -g)" -e HOME=/tmp)
else
  USER_ARGS=(--userns=keep-id -e HOME=/tmp)
fi

# No network: the stage seeds the lyric cache, so a take never asks lrclib, and
# with no network it cannot. The sound server's socket and cookie live in the
# container's /tmp, which is where stage.sh's `PULSE_COOKIE` and
# `XDG_RUNTIME_DIR` point once HOME is /tmp.
for t in "${TAPES[@]}"; do
  echo "── $t.tape"
  "$ENGINE" run --rm --network none "${USER_ARGS[@]}" \
    -e XDG_RUNTIME_DIR=/tmp/runtime \
    -v "$(cd .. && pwd):/work/vibox:Z" -w /work/vibox/demo \
    --entrypoint bash "$IMAGE" \
    -c "mkdir -m 700 /tmp/runtime \
      && pulseaudio --daemonize=yes -n --exit-idle-time=-1 \
           --load=module-null-sink --load=module-native-protocol-unix \
      && ./stage.sh up > /dev/null && vhs $t.tape > /dev/null; s=\$?; ./stage.sh down > /dev/null; exit \$s"
done
