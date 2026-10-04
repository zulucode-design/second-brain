# Work reports

This directory keeps readable HTML records of reviews, remediation plans, and ticket
recaps. They are committed evidence, not temporary exports.

When work changes a report's findings, verification status, or remaining steps, update
that report in the same pull request. Keep historical findings intact and label their
current status instead of rewriting the record as if the issue never existed.

## Evidence rules

These bind every report that records a verification result.

**Name the candidate commit for every gate result.** A gate passes on one build of one commit, so the
commit is part of the result, not context for it. Where a row relies on evidence from an earlier
candidate, say which commit and why it still applies. Four green rows proved on four different
candidates are not one green gate.

**A count ships with the command that regenerates it and the population it counted.** "42 reports"
means nothing without both; `git ls-files docs/reports | grep -v evidence/ | wc -l`, direct children
only, means something a reader can check.

**A measurement ships with its conditions.** The machine, the fixture, the build, and anything running
beside it. Two numbers measured under different conditions cannot be compared, and a regression
between them cannot be attributed.

**Record how strong a claim's proof is when it is weak.** Where a claim rests on reasoning or a cited
line rather than a run, say so plainly and say what would settle it. A row that reads the same whether
it was reproduced in the installed app or argued from source hides the difference that matters.
