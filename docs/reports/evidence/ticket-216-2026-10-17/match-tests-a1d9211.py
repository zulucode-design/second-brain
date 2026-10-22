# Run from the repository root; JSON output regenerates named-tests-a1d9211.json.
import json
import pathlib
import re
import subprocess

candidate = "a1d921143c73a2fb255ab5f9a3d2068883f10181"
main = "169b0f2d48c5ab6d7c3c23df7fc0c1da0321e1c8"
old_main = "1e99ab122dcdb873c17b7055e1cb8e148b592de2"
root = pathlib.Path(__file__).parent
manifest = json.loads((root / "named-tests-eb59fe3.json").read_text())
names = {entry["name"]: entry for entry in manifest}
assert len(manifest) == len(names) == 51
pattern = r"#\[(?:tokio::)?test(?:\([^\n]*\))?\]\s*(?:#\[[^\n]+\]\s*)*(?:async\s+)?fn\s+(\w+)"


def git(*args):
    return subprocess.check_output(["git", *args], text=True)


def added_tests(base, head):
    added = []
    for filename in git("diff", "--name-only", base, head).splitlines():
        if filename.endswith(".rs"):
            before = git("show", base + ":" + filename)
            after = git("show", head + ":" + filename)
            added.extend(sorted(set(re.findall(pattern, after)) - set(re.findall(pattern, before))))
    return added


added = added_tests(main, candidate)
assert len(added) == 51
assert set(added) == {name.rsplit("::", 1)[1] for name in names}
main_added = added_tests(old_main, main)
assert len(main_added) == 7
main_names = ["sync_sidecar::tests::" + name for name in sorted(main_added)]
previous = json.loads((root / "verification-eb59fe3.json").read_text())
result = {
    "candidate": candidate,
    "main": main,
    "previousCandidate": previous["candidate"],
    "branchAddedRustTests": 51,
    "addedFunctionsExactlyMatchManifest": True,
    "machines": {},
}
for machine, filename in [("fedora", "fedora-verify-a1d9211.txt"), ("windows", "windows-test-rust-a1d9211.txt")]:
    log = (root / filename).read_text()
    expected = [entry for entry in manifest if machine == "fedora" or entry["platform"] == "both"]
    excluded = [] if machine == "fedora" else [entry for entry in manifest if entry["platform"] == "unix"]
    matches = []
    for entry in expected:
        count = len(re.findall(r"^test " + re.escape(entry["name"]) + r" \.\.\. ok$", log, re.M))
        assert count == 1, (machine, entry["name"], count)
        matches.append({"name": entry["name"], "result": "PASS", "successfulLogLines": count})
    for entry in excluded:
        assert not re.search(r"^test " + re.escape(entry["name"]) + r" \.\.\.", log, re.M)
    for name in main_names:
        assert len(re.findall(r"^test " + re.escape(name) + r" \.\.\. ok$", log, re.M)) == 1, (machine, name)
    counts = re.findall(r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out", log, re.M)
    passed, failed, ignored, measured, filtered = map(int, counts[0])
    old_counts = previous["fedora"]["verify"]["rust"] if machine == "fedora" else previous["windows"]["testRust"]
    assert passed - old_counts["passed"] == 7
    assert ignored == old_counts["ignored"] and failed == measured == filtered == 0
    assert len(matches) == (51 if machine == "fedora" else 46)
    result["machines"][machine] = {
        "log": filename,
        "namedRustPassed": len(matches),
        "unixOnlyNotCompiled": [entry["name"] for entry in excluded],
        "rust": {"listed": passed + ignored, "passed": passed, "failed": failed, "ignored": ignored, "filtered": filtered},
        "previousRustPassed": old_counts["passed"],
        "mergeAddedRustPassed": passed - old_counts["passed"],
        "mainAddedTests": [{"name": name, "result": "PASS"} for name in main_names],
        "namedTests": matches,
    }
    if machine == "fedora":
        js_name = "a new note left by a vault switch is checked in the vault it was opened in"
        assert re.search(r"^✔ " + re.escape(js_name) + r" \(", log, re.M)
        assert [tuple(map(int, row)) for row in counts[1:]] == [(0, 0, 0, 0, 0)] * 2
        result["machines"][machine]["namedJavascriptTest"] = {"name": js_name, "result": "PASS"}
print(json.dumps(result, indent=2))
