# Run from the repository root; stdout regenerates named-tests-8326def.json.
import json
import pathlib
import re
import subprocess

candidate = "8326def68cb2c37bcfe5ea42b26e80da63a31768"
main = "169b0f2d48c5ab6d7c3c23df7fc0c1da0321e1c8"
root = pathlib.Path(__file__).parent
manifest = json.loads((root / "named-tests-eb59fe3.json").read_text())
names = {entry["name"] for entry in manifest}
assert len(manifest) == len(names) == 51
pattern = r"#\[(?:tokio::)?test(?:\([^\n]*\))?\]\s*(?:#\[[^\n]+\]\s*)*(?:async\s+)?fn\s+(\w+)"


def git(*args):
    return subprocess.check_output(["git", *args], text=True)


added = []
for filename in git("diff", "--name-only", main, candidate).splitlines():
    if filename.endswith(".rs"):
        before = git("show", main + ":" + filename)
        after = git("show", candidate + ":" + filename)
        added.extend(sorted(set(re.findall(pattern, after)) - set(re.findall(pattern, before))))
assert len(added) == 51
assert set(added) == {name.rsplit("::", 1)[1] for name in names}

log_name = "fedora-verify-8326def.txt"
log = (root / log_name).read_text()
matches = []
for entry in manifest:
    count = len(re.findall(r"^test " + re.escape(entry["name"]) + r" \.\.\. ok$", log, re.M))
    assert count == 1, (entry["name"], count)
    matches.append({"name": entry["name"], "result": "PASS", "successfulLogLines": count})
counts = re.findall(
    r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out",
    log,
    re.M,
)
assert [tuple(map(int, row)) for row in counts] == [
    (745, 0, 10, 0, 0), (0, 0, 0, 0, 0), (0, 0, 0, 0, 0)
]
js_name = "a new note left by a vault switch is checked in the vault it was opened in"
assert re.search(r"^✔ " + re.escape(js_name) + r" \(", log, re.M)
previous_name = "named-tests-a1d9211.json"
previous = json.loads((root / previous_name).read_text())
windows = previous["machines"]["windows"]
assert windows["namedRustPassed"] == 46
assert len(windows["unixOnlyNotCompiled"]) == 5

result = {
    "candidate": candidate,
    "main": main,
    "branchAddedRustTests": 51,
    "addedFunctionsExactlyMatchManifest": True,
    "regeneration": "python3 docs/reports/evidence/ticket-216-2026-10-17/match-tests-8326def.py",
    "machines": {
        "fedora": {
            "candidate": candidate,
            "log": log_name,
            "namedRustPassed": len(matches),
            "rust": {"listed": 755, "passed": 745, "failed": 0, "ignored": 10, "filtered": 0},
            "namedTests": matches,
            "namedJavascriptTest": {"name": js_name, "result": "PASS"},
        },
        "windows": {
            "rerun": False,
            "reason": "One SSH connection attempt timed out; no retry.",
            "lastCandidate": previous["candidate"],
            "lastMatch": previous_name,
            "lastLog": windows["log"],
            "lastNamedRustPassed": windows["namedRustPassed"],
            "lastRust": windows["rust"],
            "unixOnlyNotCompiled": windows["unixOnlyNotCompiled"],
        },
    },
}
print(json.dumps(result, indent=2))
