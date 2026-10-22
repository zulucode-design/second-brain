# Run from the repository root; stdout regenerates named-tests-cc7caa9.json.
import json
import pathlib
import re
import subprocess

candidate = "cc7caa9d245bd2f1bb5cb7555b31eaa6f148de37"
main = "169b0f2d48c5ab6d7c3c23df7fc0c1da0321e1c8"
root = pathlib.Path(__file__).parent
manifest = json.loads((root / "named-tests-eb59fe3.json").read_text())
new_name = (
    "semantic_search::tests::"
    "a_deferred_note_moved_to_a_name_that_is_not_unicode_stays_kept_and_the_next_check_runs"
)
manifest.append({"name": new_name, "platform": "both"})
names = {entry["name"] for entry in manifest}
assert len(manifest) == len(names) == 52
assert sum(entry["platform"] == "unix" for entry in manifest) == 5
pattern = r"#\[(?:tokio::)?test(?:\([^\n]*\))?\]\s*(?:#\[[^\n]+\]\s*)*(?:async\s+)?fn\s+(\w+)"


def git(*args):
    return subprocess.check_output(["git", *args], text=True)


added = []
for filename in git("diff", "--name-only", main, candidate).splitlines():
    if filename.endswith(".rs"):
        before = git("show", main + ":" + filename)
        after = git("show", candidate + ":" + filename)
        added.extend(sorted(set(re.findall(pattern, after)) - set(re.findall(pattern, before))))
assert len(added) == 52
assert set(added) == {name.rsplit("::", 1)[1] for name in names}

log_name = "fedora-verify-cc7caa9.txt"
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
    (746, 0, 10, 0, 0), (0, 0, 0, 0, 0), (0, 0, 0, 0, 0)
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
    "branchAddedRustTests": 52,
    "addedFunctionsExactlyMatchManifest": True,
    "regeneration": "python3 docs/reports/evidence/ticket-216-2026-10-17/match-tests-cc7caa9.py",
    "newRegression": {
        "name": new_name,
        "fedora": "PASS; Unix non-UTF-8 filename variant",
        "windows": "NOT RUN anywhere yet; unpaired-surrogate filename variant",
    },
    "machines": {
        "fedora": {
            "candidate": candidate,
            "log": log_name,
            "namedRustPassed": len(matches),
            "rust": {"listed": 756, "passed": 746, "failed": 0, "ignored": 10, "filtered": 0},
            "namedTests": matches,
            "namedJavascriptTest": {"name": js_name, "result": "PASS"},
        },
        "windows": {
            "rerun": False,
            "reason": "One SSH connection attempt timed out; no retry.",
            "applicableNamedRustTests": 47,
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
