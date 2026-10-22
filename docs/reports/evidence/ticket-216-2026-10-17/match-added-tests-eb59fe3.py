# Run from the repository root: python3 docs/reports/evidence/ticket-216-2026-10-17/match-added-tests-eb59fe3.py
import json
import pathlib
import re
import subprocess

base = "1e99ab122dcdb873c17b7055e1cb8e148b592de2"
candidate = "eb59fe3db78591d0ae10bfccffc0fe24ee1d77e9"
root = pathlib.Path(__file__).parent
names = {entry["name"]: entry for entry in json.loads((root / "named-tests-eb59fe3.json").read_text())}
files = subprocess.check_output(["git", "diff", "--name-only", base + "..." + candidate], text=True).splitlines()
pattern = r"#\[(?:tokio::)?test(?:\([^\n]*\))?\]\s*(?:#\[[^\n]+\]\s*)*(?:async\s+)?fn\s+(\w+)"
added = []
for filename in files:
    if not filename.endswith(".rs"):
        continue
    old = subprocess.check_output(["git", "show", base + ":" + filename], text=True)
    current = subprocess.check_output(["git", "show", candidate + ":" + filename], text=True)
    for function in sorted(set(re.findall(pattern, current)) - set(re.findall(pattern, old))):
        matches = [name for name in names if name.endswith("::" + function)]
        assert len(matches) == 1, (filename, function, matches)
        added.append(matches[0])
assert set(added) == set(names) and len(added) == 51
print(json.dumps({"newRustTests": len(added), "matchesRequestedManifest": True}))
