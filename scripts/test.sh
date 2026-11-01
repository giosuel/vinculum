#!/usr/bin/env bash
# The whole test suite, including the tests against a mock NetworkManager and a mock BlueZ
# (crates/nm/tests/mock_nm.rs, crates/bt/tests/mock_bluez.rs). Those need dbus-daemon and python-dbusmock; without them this
# fails instead of skipping them. Extra arguments go to cargo test.
#
# The keyring tests (crates/keyring/tests/keyring.rs) also run against a real Secret Service:
# the bus in $LITUUS_KEYRING_BUS, or else a private gnome-keyring when it's installed.
#
# The sound tests (crates/sound/tests/pipewire.rs) run against a private PipeWire with
# WirePlumber and pipewire-pulse, and a few made-up outputs and inputs, when those are installed.
set -euo pipefail
cd "$(dirname "$0")/.."
python="${LITUUS_PYTHON:-python3}"
cleanups=()
trap 'for c in "${cleanups[@]}"; do eval "$c"; done' EXIT
command -v dbus-daemon >/dev/null || { echo "test.sh: dbus-daemon is missing" >&2; exit 1; }
if ! "$python" -c 'import dbusmock' 2>/dev/null; then
  echo "test.sh: python-dbusmock is missing for $python (apt install python3-dbusmock, or pip install" \
    "python-dbusmock in a venv and set LITUUS_PYTHON to its python)" >&2
  exit 1
fi
if [ -z "${LITUUS_KEYRING_BUS:-}" ] && command -v gnome-keyring-daemon >/dev/null; then
  keyring=$(mktemp -d)
  dbus-daemon --session --address="unix:path=$keyring/bus" --nofork --nopidfile &
  bus=$!
  # gnome-keyring-daemon exits with its bus.
  cleanups+=("kill $bus 2>/dev/null; rm -rf '$keyring'")
  for _ in $(seq 100); do [ -S "$keyring/bus" ] && break; sleep 0.05; done
  printf test | DBUS_SESSION_BUS_ADDRESS="unix:path=$keyring/bus" HOME="$keyring" XDG_RUNTIME_DIR="$keyring" \
    gnome-keyring-daemon --unlock --components=secrets >/dev/null
  export LITUUS_KEYRING_BUS="unix:path=$keyring/bus"
elif [ -z "${LITUUS_KEYRING_BUS:-}" ]; then
  echo "test.sh: no gnome-keyring-daemon, so the Secret Service test is skipped" >&2
fi
if command -v pipewire >/dev/null && command -v wireplumber >/dev/null && command -v pipewire-pulse >/dev/null; then
  # A short path: PipeWire's socket paths must fit in 108 bytes.
  pw=$(mktemp -d /tmp/lpw.XXXXXX)
  pwenv=(env XDG_RUNTIME_DIR="$pw" PIPEWIRE_RUNTIME_DIR="$pw" PULSE_RUNTIME_PATH="$pw/pulse" XDG_CONFIG_HOME="$pw/config"
    XDG_STATE_HOME="$pw/state" DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent)
  "${pwenv[@]}" pipewire >"$pw/pipewire.log" 2>&1 &
  pids="$!"
  for _ in $(seq 100); do [ -S "$pw/pipewire-0" ] && break; sleep 0.05; done
  "${pwenv[@]}" wireplumber >"$pw/wireplumber.log" 2>&1 &
  pids="$pids $!"
  "${pwenv[@]}" pipewire-pulse >"$pw/pulse.log" 2>&1 &
  pids="$pids $!"
  cleanups+=("kill $pids 2>/dev/null; rm -rf '$pw'")
  for _ in $(seq 100); do [ -S "$pw/pulse/native" ] && break; sleep 0.05; done
  node() {
    "${pwenv[@]}" pw-cli create-node adapter "{ factory.name=support.null-audio-sink node.name=$1 node.description=$2 media.class=$3 audio.position=[FL FR] object.linger=true }" >/dev/null
  }
  node speakers Speakers Audio/Sink
  node tv TV Audio/Sink
  node mic Microphone Audio/Source
  export PULSE_SERVER="unix:$pw/pulse/native" LITUUS_SOUND_TEST=1
else
  echo "test.sh: no PipeWire (pipewire, wireplumber, pipewire-pulse), so the sound tests are skipped" >&2
fi
LITUUS_DBUSMOCK=1 LITUUS_PYTHON="$python" cargo test --workspace --locked "$@"
