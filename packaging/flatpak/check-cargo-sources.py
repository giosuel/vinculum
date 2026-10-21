"""Checks that cargo-sources.json has exactly the crates in Cargo.lock (name, version, checksum).
The Flatpak builds offline, so a crate missing there breaks it; CI runs this on every PR, because
the Flatpak build itself only runs on packaging changes.

    python3 -I packaging/flatpak/check-cargo-sources.py
"""
import json
import pathlib
import sys
import tomllib

root = pathlib.Path(__file__).resolve().parents[2]
lock = tomllib.loads((root / "Cargo.lock").read_text())
wanted = {
    (p["name"], p["version"], p["checksum"])
    for p in lock["package"]
    if p.get("source", "").startswith("registry+")
}
sources = json.loads((root / "packaging/flatpak/cargo-sources.json").read_text())
have = set()
for s in sources:
    if s.get("type") == "archive":
        # .../crates/<name>/<name>-<version>.crate (versions can contain dashes themselves)
        name, file = s["url"].rsplit("/", 2)[1:]
        version = file.removesuffix(".crate").removeprefix(name + "-")
        have.add((name, version, s["sha256"]))
missing, extra = sorted(wanted - have), sorted(have - wanted)
for name, version, _ in missing:
    print(f"missing from cargo-sources.json: {name} {version}")
for name, version, _ in extra:
    print(f"not in Cargo.lock any more: {name} {version}")
if missing or extra:
    print("Run packaging/flatpak/update-cargo-sources.sh and commit cargo-sources.json.")
    sys.exit(1)
print(f"cargo-sources.json: ok ({len(have)} crates)")
