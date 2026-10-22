# Run from the repository root: python3 docs/reports/evidence/ticket-216-2026-10-17/match-tests-eb59fe3.py
import json
import pathlib
import re

root = pathlib.Path(__file__).parent
entries = json.loads((root / "named-tests-eb59fe3.json").read_text())
counts = {}
for machine, filename in [("fedora", "fedora-verify-eb59fe3.txt"), ("windows", "windows-test-rust-eb59fe3.txt")]:
    log = (root / filename).read_text()
    expected = [entry for entry in entries if machine == "fedora" or entry["platform"] == "both"]
    for entry in expected:
        assert re.search(r"^test " + re.escape(entry["name"]) + r" \.\.\. ok$", log, re.M), entry["name"]
    counts[machine] = len(expected)
assert "a new note left by a vault switch is checked in the vault it was opened in" in (root / "fedora-verify-eb59fe3.txt").read_text()
print(json.dumps(counts))
