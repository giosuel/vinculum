#!/usr/bin/env bash
# The whole test suite, including the tests against a mock NetworkManager
# (crates/nm/tests/mock_nm.rs). Those need dbus-daemon and python-dbusmock; without them this
# fails instead of skipping them. Extra arguments go to cargo test.
set -euo pipefail
cd "$(dirname "$0")/.."
python="${VINCULUM_PYTHON:-python3}"
command -v dbus-daemon >/dev/null || { echo "test.sh: dbus-daemon is missing" >&2; exit 1; }
if ! "$python" -c 'import dbusmock' 2>/dev/null; then
  echo "test.sh: python-dbusmock is missing for $python (apt install python3-dbusmock, or pip install" \
    "python-dbusmock in a venv and set VINCULUM_PYTHON to its python)" >&2
  exit 1
fi
VINCULUM_DBUSMOCK=1 VINCULUM_PYTHON="$python" exec cargo test --workspace --locked "$@"
