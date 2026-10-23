---
name: kiro-workflow
description: "Kiro workflow for first passes and fix attempts: the brief, exchange branch, Kiro session, handoff, import, intake, review and publication. Use whenever a task involves Kiro: a Kiro brief or handoff, a `kiro/*` branch, a pull request Kiro opened, the Kiro mirror, an issue given to Kiro, or a review, test or CI finding to fix."
---

# Kiro workflow

Kiro, Amazon's coding agent, writes the first pass of an issue and, by default, the fixes after it,
in a cloud sandbox. That sandbox sees only the GitHub repository Kiro is connected to. It has no
access to Nicolas's Fedora machine, his Windows machine, or anything else on them.

Everything else belongs to Claude and Sol, as `AGENTS.md` assigns it. Read its **Roles** and
**Pull requests** sections rather than working from memory of them; they still bind. Roles says
which fixes go to Kiro and which stay with Claude. This skill adds only what is specific to Kiro.

## Who does what

- **Kiro** writes the code and tests each brief asks for: a first pass, or the fixes of a fix
  attempt. It commits its own work and pushes only its own working branch. It never merges anything.
- **Claude** grills Nicolas before a first-pass brief, triages each round of findings, writes and
  commits every brief, starts and resumes every Kiro session, runs the watcher, imports and reads
  Kiro's work, and makes the fixes `AGENTS.md` Roles keeps with Claude. Claude never pushes.
- **Sol** makes every push from this machine, the exchange branch included, keeps the mirror in step,
  and runs every check, test and review, at `xhigh` reasoning effort. Sol commits the report, opens
  the pull request, watches CI and merges after Nicolas's go-ahead.
- **Nicolas** settles scope in the grilling, may send any fix to Claude or to agents Claude directs,
  starts a Kiro session by hand only when the CLI cannot (section 3), and gives the go-ahead to
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
- has its main set by Sol to the pinned main of this repository before each task or set of parallel
  tasks, at a commit that contains this skill. Mirror main then stays frozen until the last open task
  closes: protected where GitHub allows it, and checked against the pinned commit at intake.
- is never a source for main. Claude imports only the frozen exchange head, never mirror main or a
  merge commit from a mirror pull request.
- never carries closing keywords. Commit messages and pull request text in the mirror use no form of
  "close", "fix" or "resolve" next to an issue reference. Issue closure goes into the reviewed pull
  request on this repository, added by Sol.
- follows the same privacy rules as this public repository.

## 1. Brief

Kiro tasks may run in parallel when their issues touch different files and features. Parallel tasks
share one `base_sha`, their briefs' allowed files do not overlap, and each brief names the tasks
running beside it. Each task still has its own exchange branch, handoff, delivery branch and pull
request.

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

Sol pushes the exchange branch, in mirror mode after setting mirror main to `base_sha`. Claude then
starts Kiro (section 3).

### Fix attempts

A fix attempt carries one round of findings to Kiro: from a review round, the final review, one of
Sol's test runs, or CI. Claude triages the round first. Each finding is either fixed, or left unfixed
with a follow-up issue as `AGENTS.md` **Pull requests** allows. A finding that needs a decision on
scope, security, data loss or the product contract goes to Nicolas before the brief. One fix attempt
then carries every finding of the round that Roles gives to Kiro, from both review axes.

A fix attempt follows the steps above, except:

- Its `base_sha` is the head of the delivery branch `fix/<N>-<slug>`, and its exchange branch starts
  there. Sol pushes that branch to the working repository with the brief commit on top; in mirror
  mode, mirror main stays where it is.
- Its brief quotes each finding verbatim and says that `docs/reports/` stays untouched. It may link
  the issue instead of quoting it when an earlier brief for the issue quoted it.

## 2. Kiro's rules

Kiro, this section is yours.

- Read `AGENTS.md`, this skill, the brief, and the parts of `docs/SPEC.md` and `CONTEXT.md` the brief
  points to.
- One session may receive several briefs for an issue, one per attempt. Work from the newest brief;
  where it differs from what you remember of an earlier attempt, the brief wins.
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

## 3. Kiro sessions and the watcher

Claude starts every Kiro session from this machine with the Kiro CLI, in Kiro's cloud sandbox:

```bash
kiro-cli chat --no-interactive --agent-engine v3 --cloud --repo <working repository> \
  --model claude-opus-5.5 --effort xhigh "<prompt>" < /dev/null
```

- The prompt names the exchange branch and the brief, and asks Kiro to follow this skill and the
  brief.
- Every session runs `claude-opus-5.5` at `xhigh` effort unless Nicolas names another model or
  effort. A session keeps its model and effort for its whole life.
- One session per issue. The issue's first attempt starts it, and the CLI prints its ID as
  `[cloud] session <id>`. Claude keeps that ID in its notes for the task, outside the repository, and
  starts every later attempt by resuming the session:
  `kiro-cli chat --no-interactive --agent-engine v3 --resume-id <id> "<prompt>" < /dev/null`.
  If the resume fails, Claude starts a fresh session with the same prompt; the brief carries
  everything Kiro needs.
- The session runs in Kiro's cloud and keeps working after the local CLI exits or this machine
  sleeps. The CLI exiting is the quick sign that Kiro has finished; the watcher is the reliable one.
- If the CLI cannot start or resume a session, Claude tells Nicolas. He starts one by hand on the
  working repository, with the exchange branch as its base branch, and asks Kiro to follow this skill
  and the brief.

While any Kiro session is working on an attempt, Claude runs one background shell loop against the
working repository, covering every such attempt:

- `git fetch` every ref, then list the refs that changed since the last poll, not only `kiro/*`.
  `git ls-remote` alone does not update local refs, so fetch before reading a handoff.
- Read the handoff on each changed branch whose history contains an open task's `brief_sha`, and list
  open pull requests whose head contains one, whoever authored them. Kiro can open pull requests as
  `app/kiro-agent` or, if a setting allows it, as Nicolas's GitHub user.
- Wake Claude only on a change: `ready_for_claude` or `question`, a new branch, a pull request, or a
  push after the handoff froze.
- Retry a failed fetch or API call with backoff, and report it once it keeps failing. A failure is
  never read as "nothing changed".
- After 24 hours with no push, tell Nicolas once.

The watcher stops once no session is working on an attempt. If Claude's session restarts, Claude
restarts the watcher from the pinned commits.

## 4. Questions

On `state: question`, Claude answers from the issue and `docs/SPEC.md` where they settle it, and
otherwise asks Nicolas. The answer goes into a new brief as the next attempt, since a brief never
changes. Kiro may also stop without pushing. Claude reads its last message in the CLI output, or by
resuming the session, and the watcher's 24-hour notice covers a stop nobody saw.

## 5. Freeze and import

On `ready_for_claude`, Claude freezes the attempt at the handoff commit `H`:

- `H` changes only the handoff file, and `H^` is the `code_head` its frontmatter names.
- `brief_sha` is an ancestor of `H`, and `base_sha` of `brief_sha`, with the brief file unchanged.
- In mirror mode, mirror main still equals the commit Sol last set it to. For a first pass, that
  commit is `base_sha`.
- For a fix attempt, `base_sha` is the delivery branch head the brief names, and the delivery branch
  still ends there.

Claude does not import later pushes. It reads them, and if they matter, the next attempt covers them.

Claude creates the delivery branch `fix/<N>-<slug>` in a worktree at `base_sha` on this repository.
For a fix attempt, the delivery branch already ends at `base_sha`, and Claude imports onto it.
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
3. Sol's review loop on the exact head, Spec and Standards separately, at `xhigh`. Claude triages
   each round and routes its fixes as `AGENTS.md` Roles says: by default a fix attempt to Kiro
   (section 1), frozen, imported and read like a first pass (sections 5 and 6). When a finding comes
   back after Claude's own fix, or three rounds pass without a clean result, Claude writes a
   diagnosis and takes it to Nicolas.
4. Sol merges `origin/main` once. Conflicts go to Claude.
5. Sol's full verification on that head, on both machines: `pnpm verify` and the machine and package
   checks the brief names. Sol matches the tests the change adds against the tests that actually ran.
   A new Rust test marked `#[ignore]` runs by name through `pnpm test:rust`, which passes its
   arguments on to `cargo test`; otherwise Sol records the acceptance it covers as unverified.
   Failures go to Claude, who routes their fixes as in step 3.
6. Sol commits the report, recording the candidate commit, the review rounds, and for each round
   who made its fixes (Kiro's attempt, Claude, or agents Claude directed) and the fix commits.
7. The final review of that exact head.
8. Sol pushes and opens the pull request with its `Final review: <sha>` line, and adds the closing
   reference when the attempt completes the issue. Before merge, Sol checks the pull request's
   `closingIssuesReferences`, since a sentence like "does not close #N" still closes it.
9. CI. A failure goes to Claude, who routes its fix as in step 3; Sol reruns the affected checks and
   a fresh review of the whole branch's exact head before the next push.
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

After three Kiro issues, Claude reports, for each one, the fix commits after import by who made them
(Kiro, Claude, or agents Claude directed) and the number of review rounds, taken from git and the
committed reports.

## 10. First task in each mode

Record, from the pilot:

- whether Kiro loaded this skill by itself or read it through the link in `AGENTS.md`;
- whether it worked on the exchange branch or on its own branch from it;
- whether it delivered by pushing a branch without opening a pull request;
- whether the handoff followed the template;
- whether the watcher woke Claude, survived a restart and handled an API failure;
- which checks Kiro ran in its sandbox, and the sandbox's network and secrets settings.

Mirror mode, recorded. In #158, #169 and #216, Kiro worked on the exchange branch, delivered by
pushing with no pull request, and followed the handoff template. In #158 the watcher woke Claude and
survived restarts, no API failure came up, and the sandbox was Amazon Linux without GTK, so
`pnpm test:rust` could not run there. In the CLI tests of 2026-10-23, Kiro read this skill directly
rather than loading it natively, and the sandbox reported open outbound network.

## Privacy

The same rules cover this public repository and the private mirror. Briefs, handoffs, commits, pull
request text and evidence carry no hostnames, private machine paths, note content, credentials or
personal data. Nicolas checks that Kiro's environment holds no secrets, environment variables, cloud
roles or connected tools beyond what the task needs. The Kiro CLI on this machine has no MCP servers
configured, and stays that way: the CLI reports MCP isolation for cloud sessions as unconfirmed.

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
