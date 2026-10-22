#!/usr/bin/env bash
# Inside test.sh's container: start the installed Flatpak on headless sway, with no
# NetworkManager around, and check that its window opens (it says NetworkManager isn't running).
set -euo pipefail
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
printf 'output HEADLESS-1 resolution 1280x800\n' > /tmp/sway.conf
sway -c /tmp/sway.conf >/tmp/sway.log 2>&1 &
sway=$!
app=""
trap 'kill $app $sway 2>/dev/null || true' EXIT
for _ in $(seq 50); do ls "$XDG_RUNTIME_DIR"/sway-ipc.*.sock >/dev/null 2>&1 && break; sleep 0.2; done
SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*.sock)
WAYLAND_DISPLAY=$(ls "$XDG_RUNTIME_DIR" | grep -m1 '^wayland-[0-9]*$')
export SWAYSOCK WAYLAND_DISPLAY
# Its log (portal and GPU warnings, in a container without either) is shown only on failure.
flatpak run ch.giosuel.Vinculum >/tmp/app.log 2>&1 &
app=$!
for _ in $(seq 100); do
  swaymsg -t get_tree | grep -q '"app_id": *"ch.giosuel.Vinculum"' && break
  sleep 0.2
done
swaymsg -t get_tree | grep -q '"app_id": *"ch.giosuel.Vinculum"' || { echo "error: no window" >&2; tail -20 /tmp/app.log /tmp/sway.log; exit 1; }
echo "Window open: $(swaymsg -t get_tree | grep -o '"name": *"Vinculum[^"]*"' | head -1)"
