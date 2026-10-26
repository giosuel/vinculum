#!/usr/bin/env bash
# The whole test suite, including the tests against a mock NetworkManager and a mock BlueZ
# (crates/nm/tests/mock_nm.rs, crates/bt/tests/mock_bluez.rs). Those need dbus-daemon and python-dbusmock; without them this
# fails instead of skipping them. Extra arguments go to cargo test.
#
# The keyring tests (crates/keyring/tests/keyring.rs) also run against a real Secret Service:
# the bus in $VINCULUM_KEYRING_BUS, or else a private gnome-keyring when it's installed.
set -euo pipefail
cd "$(dirname "$0")/.."
python="${VINCULUM_PYTHON:-python3}"
command -v dbus-daemon >/dev/null || { echo "test.sh: dbus-daemon is missing" >&2; exit 1; }
if ! "$python" -c 'import dbusmock' 2>/dev/null; then
  echo "test.sh: python-dbusmock is missing for $python (apt install python3-dbusmock, or pip install" \
    "python-dbusmock in a venv and set VINCULUM_PYTHON to its python)" >&2
  exit 1
fi
if [ -z "${VINCULUM_KEYRING_BUS:-}" ] && command -v gnome-keyring-daemon >/dev/null; then
  keyring=$(mktemp -d)
  dbus-daemon --session --address="unix:path=$keyring/bus" --nofork --nopidfile &
  bus=$!
  # gnome-keyring-daemon exits with its bus.
  trap 'kill $bus 2>/dev/null; rm -rf "$keyring"' EXIT
  for _ in $(seq 100); do [ -S "$keyring/bus" ] && break; sleep 0.05; done
  printf test | DBUS_SESSION_BUS_ADDRESS="unix:path=$keyring/bus" HOME="$keyring" XDG_RUNTIME_DIR="$keyring" \
    gnome-keyring-daemon --unlock --components=secrets >/dev/null
  export VINCULUM_KEYRING_BUS="unix:path=$keyring/bus"
elif [ -z "${VINCULUM_KEYRING_BUS:-}" ]; then
  echo "test.sh: no gnome-keyring-daemon, so the Secret Service test is skipped" >&2
fi
VINCULUM_DBUSMOCK=1 VINCULUM_PYTHON="$python" cargo test --workspace --locked "$@"
