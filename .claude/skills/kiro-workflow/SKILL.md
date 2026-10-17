---
name: kiro-workflow
description: "Kiro first-pass workflow: the brief, exchange branch, handoff, import, intake, fixes, review and publication. Use whenever a task involves Kiro: a Kiro brief or handoff, a `kiro/*` branch, a pull request Kiro opened, the Kiro mirror, or an issue given to Kiro."
---

# Kiro workflow

Kiro, Amazon's coding agent, writes the first pass of an issue in its own sandbox. That sandbox sees
only the GitHub repository Kiro is connected to. It has no access to Nicolas's Fedora machine, his
Windows machine, or anything else on them.

Everything after Kiro's first pass belongs to Claude and Sol, as `AGENTS.md` assigns it. Read its
**Roles** and **Pull requests** sections rather than working from memory of them; they still bind.
This skill adds only what is specific to Kiro.

## Who does what

- **Kiro** writes the first pass of the code and its tests, within the brief. It commits its own
  work and pushes only its own working branch. It never merges anything.
- **Claude** grills Nicolas before the brief, writes and commits the brief, runs the watcher, imports
  and reads Kiro's work, and writes every fix after it: intake, review, test, CI and conflict fixes.
  A fix never goes back to Kiro. Claude never pushes.
- **Sol** makes every push from this machine, the exchange branch included, keeps the mirror in step,
  and runs every check, test and review, at `xhigh` reasoning effort. Sol commits the report, opens
  the pull request, watches CI and merges after Nicolas's go-ahead.
- **Nicolas** settles scope in the grilling, starts each Kiro session, and gives the go-ahead to
  merge.

Everyone commits their own work.

## What counts as evidence

Only Sol's runs, checks and tests count. Kiro may run whatever it needs inside its sandbox while it
works, and the handoff lists those runs, but they are information only. They never stand as evidence
for a gate, a report, a review or a pull request.

Kiro never runs, claims or imitates a check that needs the Fedora or Windows machine: installed
packages, the GUI, hotkeys, Windows builds or tests, performance measurements, or sync between
machines. Those are Sol's. Kiro lists them under "What I could not check".

## Modes

| Mode | Repository Kiro is connected to | Pull requests by Kiro |
|---|---|---|
| Mirror | A private mirror of this repository, and not this repository | Allowed in the mirror; never merged |
| Direct | This repository | Never |

Work starts in mirror mode. Direct mode starts only after Kiro has shown, in mirror mode, that it
delivers by pushing a branch without opening a pull request, and Nicolas agrees to switch. Each brief
names its mode and working repository, and the mode never changes during an attempt.

The no-pull-request hard rule in `AGENTS.md` covers this repository and does not change. A pull
request Kiro opens in the mirror is not a pull request of this repository.

The mirror:

- is an independent private repository, not a GitHub fork. Kiro's GitHub app can reach the mirror and
  not this repository, and Kiro's sandbox and connected tools hold no credentials for this repository.
- has GitHub Actions disabled.
- has its main set by Sol to the pinned main of this repository before each task, at a commit that
  contains this skill. Mirror main then stays frozen until the task closes: protected where GitHub
  allows it, and checked against the pinned commit at intake.
- is never a source for main. Claude imports only the frozen exchange head, never mirror main or a
  merge commit from a mirror pull request.
- never carries closing keywords. Commit messages and pull request text in the mirror use no form of
  "close", "fix" or "resolve" next to an issue reference. Issue closure goes into the reviewed pull
  request on this repository, added by Sol.
- follows the same privacy rules as this public repository.

## 1. Brief

One Kiro task runs at a time.

Before writing the brief, Claude grills Nicolas on anything material: behaviour the issue leaves open,
scope, security, data loss and the product contract. A Kiro session is a poor place to answer
questions, so answers go into the brief.

Claude, in a worktree:

1. Creates the exchange branch `kiro/<N>-attempt-<k>` at a pinned `origin/main` commit, the
   `base_sha`.
2. Commits two files as one commit, the `brief_sha`:
   - `docs/handoffs/<N>-attempt-<k>-brief.md`, the brief.
   - `docs/handoffs/<N>-attempt-<k>-handoff.md`, the handoff template below with `state: working`.
3. Checks both files against the privacy rules.

The brief holds:

- the mode, the working repository and the `base_sha`;
- the issue URL and its text verbatim, with the clarifications Nicolas approved, and any redaction
  marked as one;
- the acceptance criteria for this attempt. When the attempt covers part of an issue, it says which
  part and that the issue stays open;
- the allowed files (paths or globs) and what is out of scope;
- the invariants from `docs/SPEC.md` and `CONTEXT.md` that the change must keep;
- the checks Sol will run and on which machine, and the Windows pitfalls in `AGENTS.md` that apply.

The brief never changes once pushed. A changed brief is a new attempt, with `<k>` raised by one.

Sol pushes the exchange branch, in mirror mode after setting mirror main to `base_sha`. Nicolas then
starts a Kiro session on the working repository, with the exchange branch chosen as the base branch,
and asks Kiro to follow the `kiro-workflow` skill and the brief.

## 2. Kiro's rules

Kiro, this section is yours.

- Read `AGENTS.md`, this skill, the brief, and the parts of `docs/SPEC.md` and `CONTEXT.md` the brief
  points to.
- Work on the exchange branch or on a branch created from it. Your branch must contain the brief
  commit unchanged.
- Change only the files the brief allows. Never touch `.github/`, `.gitignore`, `AGENTS.md`,
  `CLAUDE.md`, `.claude/`, `.agents/`, `.kiro/`, `docs/reports/`, lockfiles, dependency lists in
  `package.json` or `Cargo.toml`, or any file under `docs/handoffs/` other than your handoff, unless
  the brief names it.
- Make one commit per logical step, and fix a mistake with a new commit. Never amend, rebase, squash
  or force-push a commit you have pushed, and never merge main or any other branch into yours.
- Write no closing keyword next to an issue reference, in commit messages or pull request text.
- Put new tests where `pnpm test` and `pnpm test:rust` find them. A Rust test marked `#[ignore]`
  runs only when asked, so name it in the handoff.
- Run what you like in your sandbox and record the commands and results in the handoff. Anything that
  needs the Fedora or Windows machine goes under "What I could not check"; never imitate it.
- Take the conservative reading of a reversible choice within scope, and record it under
  Assumptions. Stop only when the work would need a scope change, or a decision about security, data
  loss or the product contract that the brief does not settle. Then set `state: question`, write the
  question, commit and push.
- Keep secrets, hostnames, private machine paths, note content and personal data out of code,
  commits, the handoff and pull request text. Repository-relative paths are fine.
- Make your last commit change the handoff file only, with `state: ready_for_claude` and `code_head`
  set to that commit's parent. Then push your branch and stop.
- Direct mode: never open a pull request. Mirror mode: you may open one in the mirror if your tools
  insist, with no closing keywords, and never merge it.
- Claude imports nothing you push after the handoff commit, so push again only if asked.

The handoff template:

```markdown
---
task_id: <N>-attempt-<k>
base_sha: <base_sha>
code_head: <parent of the handoff commit>
state: working | question | ready_for_claude
---

## Files changed and why
## Out-of-scope changes
## Commands run and results (sandbox, information only)
## What I could not check
## Assumptions
## Question
```

## 3. Watcher

While a Kiro task is open, Claude runs a background shell loop against the working repository:

- `git fetch` every ref, then list the refs that changed since the last poll, not only `kiro/*`.
  `git ls-remote` alone does not update local refs, so fetch before reading a handoff.
- Read the handoff on each changed branch whose history contains `brief_sha`, and list open pull
  requests whose head contains `brief_sha`, whoever authored them. Kiro can open pull requests as
  `app/kiro-agent` or, if a setting allows it, as Nicolas's GitHub user.
- Wake Claude only on a change: `ready_for_claude` or `question`, a new branch, a pull request, or a
  push after the handoff froze.
- Retry a failed fetch or API call with backoff, and report it once it keeps failing. A failure is
  never read as "nothing changed".
- After 24 hours with no push, tell Nicolas once.

The watcher stops when the task closes. If Claude's session restarts, Claude restarts it from the
pinned commits.

## 4. Questions

On `state: question`, Claude answers from the issue and `docs/SPEC.md` where they settle it, and
otherwise asks Nicolas. The answer goes into a new brief as the next attempt, since a brief never
changes. Kiro may also stop in its session without pushing; Nicolas sees that there, and the watcher's
24-hour notice covers it.

## 5. Freeze and import

On `ready_for_claude`, Claude freezes the attempt at the handoff commit `H`:

- `H` changes only the handoff file, and `H^` is the `code_head` its frontmatter names.
- `brief_sha` is an ancestor of `H`, and `base_sha` of `brief_sha`, with the brief file unchanged.
- In mirror mode, mirror main still equals `base_sha`.

Claude does not import later pushes. It reads them, and if they matter, the next attempt covers them.

Claude creates the delivery branch `fix/<N>-<slug>` in a worktree at `base_sha` on this repository.
`git cherry-pick -x` copies each code commit in `brief_sha..code_head`, skipping brief and handoff
commits. From a commit that mixes code with the brief or handoff, Claude imports only the code and
keeps Kiro as author. A merge commit anywhere in that range rejects the attempt. Claude removes any
closing keyword from imported messages while cherry-picking.

## 6. Intake

Intake is Claude's read, not a test run. On every imported commit and on the final diff, against the
brief:

1. Mechanical checks: `git diff --name-only` against the allowed files and the forbidden paths in
   Kiro's rules; `git check-ignore --no-index` on every added file, since a tracked file can match an
   ignore rule; new tests sit where the runners find them; no secrets, hostnames, private paths or
   personal data.
2. A read of the diff, commit by commit.
3. Fixes, committed by Claude. Claude reverts an out-of-scope or forbidden change in its own commit
   rather than arguing it.

A revert does not remove a secret from history. A leaked secret stops the task. Claude tells Nicolas,
who revokes or rotates it, and any deletion or history rewrite waits for his approval.

## 7. Fixes, review, verification and publication

`AGENTS.md` **Pull requests** governs this part. The order:

1. Claude's intake fixes, committed locally.
2. Sol's build gate: `pnpm check`, the Linux compile, and `pnpm test:rust` on Windows early.
3. Sol's review loop on the exact head, Spec and Standards separately, at `xhigh`. Claude fixes each
   round. When the same finding comes back or three rounds pass without a clean result, Claude writes
   a diagnosis and takes it to Nicolas.
4. Sol merges `origin/main` once. Conflicts go to Claude.
5. Sol's full verification on that head, on both machines: `pnpm verify` and the machine and package
   checks the brief names. Sol matches the tests the change adds against the tests that actually ran.
   A new Rust test marked `#[ignore]` runs by name through `pnpm test:rust`, which passes its
   arguments on to `cargo test`; otherwise Sol records the acceptance it covers as unverified.
6. Sol commits the report, recording the candidate commit, the review rounds and Claude's fix
   commits.
7. The final review of that exact head.
8. Sol pushes and opens the pull request with its `Final review: <sha>` line, and adds the closing
   reference when the attempt completes the issue. Before merge, Sol checks the pull request's
   `closingIssuesReferences`, since a sentence like "does not close #N" still closes it.
9. CI. A failure goes to Claude; Sol reruns the affected checks and a fresh review of the whole
   branch's exact head before the next push.
10. Claude tells Nicolas the pull request can merge, and Sol merges after his go-ahead.

If main moves again before the pull request opens, Sol merges it, reruns the affected checks and
reviews the whole branch again. Evidence carried from an earlier candidate names that commit and says
why it still applies.

Before each review or test run, check Sol's usage as `AGENTS.md` describes; at a usage limit, the
fallback it names runs that review or test instead. Pushes, the pull request, CI watching, mirror
updates and the merge wait for Sol.

## 8. A pull request opened by Kiro

- **Direct mode.** This breaches the gate. The attempt halts, Sol closes the pull request unmerged and
  never reuses it, and Sol checks its closing references and the state of every issue it names. Claude
  tells Nicolas, and work returns to mirror mode until he decides otherwise.
- **Mirror mode.** Expected and harmless. Nobody merges it. Sol closes it unmerged after import.

## 9. Measurement

After three Kiro issues, Claude reports, for each one, Claude's fix commits after import and the
number of review rounds, taken from git and the committed reports.

## 10. First task in each mode

Record, from the pilot:

- whether Kiro loaded this skill by itself or read it through the link in `AGENTS.md`;
- whether it worked on the exchange branch or on its own branch from it;
- whether it delivered by pushing a branch without opening a pull request;
- whether the handoff followed the template;
- whether the watcher woke Claude, survived a restart and handled an API failure;
- which checks Kiro ran in its sandbox, and the sandbox's network and secrets settings.

## Privacy

The same rules cover this public repository and the private mirror. Briefs, handoffs, commits, pull
request text and evidence carry no hostnames, private machine paths, note content, credentials or
personal data. Nicolas checks that Kiro's environment holds no secrets, environment variables, cloud
roles or connected tools beyond what the task needs.

## Changing this skill

This is the only skill committed to the repository; every other project skill stays local. It lives
as three byte-identical copies:

- `.claude/skills/kiro-workflow/`, the one to edit;
- `.agents/skills/kiro-workflow/`;
- `.kiro/skills/kiro-workflow/`.

After an edit, copy the whole folder over the other two. Before any push that touches them, Sol runs
the check CI also runs:

```bash
diff -r .claude/skills/kiro-workflow .agents/skills/kiro-workflow
diff -r .claude/skills/kiro-workflow .kiro/skills/kiro-workflow
```

If a copy is missing or differs, stop Kiro work until the copies match again.
