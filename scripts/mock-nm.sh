#!/usr/bin/env bash
# Runs a command against a mock NetworkManager on a private bus, with the networks from the
# mockups (connected to "Lumen Home", a few others in range, an unplugged Ethernet port):
#   scripts/mock-nm.sh vinculum-cli list
#   scripts/mock-nm.sh target/debug/vinculum
# The command sees the mock as its system bus; the real one is never touched. Needs dbus-daemon
# and python-dbusmock ($VINCULUM_PYTHON picks the interpreter that has it).
set -euo pipefail
[ $# -gt 0 ] || { echo "usage: mock-nm.sh COMMAND [ARGS...]" >&2; exit 2; }
if [ -z "${MOCK_NM_INSIDE:-}" ]; then
  # A private session bus that dbus-run-session removes again when the command exits.
  MOCK_NM_INSIDE=1 exec dbus-run-session -- "$0" "$@"
fi
python="${VINCULUM_PYTHON:-python3}"
"$python" -m dbusmock --session --template networkmanager >/dev/null 2>&1 &
mock=$!
trap 'kill $mock 2>/dev/null' EXIT
for _ in $(seq 100); do
  gdbus introspect --session -d org.freedesktop.NetworkManager -o /org/freedesktop/NetworkManager >/dev/null 2>&1 && break
  sleep 0.05
done
call() { gdbus call --session -d org.freedesktop.NetworkManager -o /org/freedesktop/NetworkManager -m "org.freedesktop.DBus.Mock.$1" "${@:2}" | sed -n "s/^('\(.*\)',)$/\1/p"; }
psk=256 # NM_802_11_AP_SEC_KEY_MGMT_PSK, the only kind the mock knows
wifi=$(call AddWiFiDevice wlan0 wlp2s0 100)
home=$(call AddAccessPoint "$wifi" home5 "Lumen Home" 3C:A6:2F:11:8E:20 2 5180 866000 78 $psk)
call AddAccessPoint "$wifi" home24 "Lumen Home" 3C:A6:2F:11:8E:21 2 2437 144000 64 $psk >/dev/null
call AddAccessPoint "$wifi" cafe "Aster Café" 08:96:AD:3E:71:C2 2 2412 54000 61 $psk >/dev/null
call AddAccessPoint "$wifi" library "Bibliothek-Free" 0A:11:22:33:44:55 2 5200 300000 40 0 >/dev/null
call AddAccessPoint "$wifi" neighbour "Nachbar_5G" 7E:22:41:90:0B:13 2 5745 400000 22 $psk >/dev/null
connection=$(call AddWiFiConnection "$wifi" home "Lumen Home" wpa-psk)
call AddActiveConnection "['$wifi']" "$connection" "$home" home 2 >/dev/null
call AddEthernetDevice eth0 enp3s0 20 >/dev/null
DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" "$@"
