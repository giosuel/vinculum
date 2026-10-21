#!/usr/bin/env bash
# Runs a command against a mock NetworkManager on a private bus, with the networks from the
# mockups (connected to "Lumen Home", a few others in range, an unplugged Ethernet port):
#   scripts/mock-nm.sh vinculum-cli list
#   scripts/mock-nm.sh target/debug/vinculum
# The command sees the mock as its system bus; the real one is never touched. Needs dbus-daemon
# and python-dbusmock ($VINCULUM_PYTHON picks the interpreter that has it).
#
# The mock also has an AgentManager, so Vinculum's secret agent registers, and a method that
# makes it ask for a password the way NetworkManager does when one is missing or wrong:
#   MOCK_NM_ADDRESS_FILE=/tmp/bus scripts/mock-nm.sh target/debug/vinculum &
#   gdbus call --address "$(cat /tmp/bus)" -d org.freedesktop.NetworkManager \
#     -o /org/freedesktop/NetworkManager -m org.freedesktop.DBus.Mock.Test.AskForPassword 'Aster Café' 1
# (flags: 1 to ask, 3 to say the last password was wrong).
set -euo pipefail
[ $# -gt 0 ] || { echo "usage: mock-nm.sh COMMAND [ARGS...]" >&2; exit 2; }
if [ -z "${MOCK_NM_INSIDE:-}" ]; then
  # A private bus that dbus-run-session removes again when the command exits. The command keeps
  # the real session bus (if any), so nothing it asks for (the desktop portal, say) starts there.
  MOCK_NM_INSIDE=1 MOCK_NM_SESSION="${DBUS_SESSION_BUS_ADDRESS:-}" exec dbus-run-session -- "$0" "$@"
fi
python="${VINCULUM_PYTHON:-python3}"
"$python" -m dbusmock --session --template networkmanager >"${MOCK_NM_LOG:-/dev/null}" 2>&1 &
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
gdbus call --session -d org.freedesktop.NetworkManager -o /org/freedesktop/NetworkManager -m org.freedesktop.DBus.Mock.AddObject \
  /org/freedesktop/NetworkManager/AgentManager org.freedesktop.NetworkManager.AgentManager '@a{sv} {}' \
  "[('RegisterWithCapabilities', 'su', '', ''), ('Unregister', '', '', '')]" >/dev/null
# AskForPassword(ssid, flags): calls GetSecrets on whichever bus name serves the agent.
ask='
import dbus
for name in self.connection.list_names():
    if not name.startswith(":") or name == self.connection.get_unique_name():
        continue
    try:
        agent = self.connection.get_object(name, "/org/freedesktop/NetworkManager/SecretAgent", introspect=False)
        dbus.Interface(agent, "org.freedesktop.DBus.Introspectable").Introspect(timeout=1)
    except dbus.exceptions.DBusException:
        continue
    settings = {
        "connection": {"id": args[0], "uuid": "mock-" + args[0], "type": "802-11-wireless"},
        "802-11-wireless": {"ssid": dbus.ByteArray(args[0].encode())},
        "802-11-wireless-security": {"key-mgmt": "wpa-psk", "psk-flags": dbus.UInt32(1)},
    }
    agent.GetSecrets(dbus.Dictionary(settings, signature="sa{sv}"), dbus.ObjectPath("/org/freedesktop/NetworkManager/Settings/99"),
        "802-11-wireless-security", dbus.Array([], signature="s"), dbus.UInt32(args[1]),
        dbus_interface="org.freedesktop.NetworkManager.SecretAgent", timeout=180,
        reply_handler=lambda s, self=self: self.log("agent answered " + str(dict(s))),
        error_handler=lambda e, self=self: self.log("agent refused: " + e.get_dbus_name()))
'
gdbus call --session -d org.freedesktop.NetworkManager -o /org/freedesktop/NetworkManager -m org.freedesktop.DBus.Mock.AddMethod \
  org.freedesktop.DBus.Mock.Test AskForPassword su '' "$ask" >/dev/null
mock_bus="$DBUS_SESSION_BUS_ADDRESS"
[ -n "${MOCK_NM_ADDRESS_FILE:-}" ] && echo "$mock_bus" > "$MOCK_NM_ADDRESS_FILE"
if [ -n "$MOCK_NM_SESSION" ]; then export DBUS_SESSION_BUS_ADDRESS="$MOCK_NM_SESSION"; else unset DBUS_SESSION_BUS_ADDRESS; fi
DBUS_SYSTEM_BUS_ADDRESS="$mock_bus" "$@"
