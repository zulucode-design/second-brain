# Run from the repository root: python3 docs/reports/evidence/ticket-216-2026-10-17/match-added-tests-32fe3af.py
import json
import pathlib
import re
import subprocess

base = "1e99ab122dcdb873c17b7055e1cb8e148b592de2"
candidate = "32fe3afdc3092e42e937fd2f6edbd09453680f7b"
root = pathlib.Path(__file__).parent
names = {entry["name"]: entry for entry in json.loads((root / "named-tests-32fe3af.json").read_text())}
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
assert set(added) == set(names) and len(added) == 50
print(json.dumps({"newRustTests": len(added), "matchesRequestedManifest": True}))
