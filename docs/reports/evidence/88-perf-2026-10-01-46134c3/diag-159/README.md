# #159: Windows diagnosis and first design pass

Installed candidate: `46134c36a99b53d17a56c3a96960b76ca7aae040`. The executable SHA-256 matched the original perf run. The existing disposable vault and machine-local index were reused. No application code, commits, pushes, or issue updates were made.

## Result

| Session | Delay after startup-ready | Five exact scans, ms | p95, ms |
| --- | ---: | --- | ---: |
| A1 | 0 | 69.4, 140.2, 71.1, 67.2, 67.3 | 140.2 |
| A2 | 0 | 68.4, 134.5, 66.4, 71.8, 71.3 | 134.5 |
| B1 | 20,000 ms | 64.2, 64.7, 65.1, 64.1, 63.7 | 65.1 |
| B2 | 20,000 ms | 64.7, 64.5, 65.8, 73.9, 67.0 | 73.9 |

All 20 queries returned 20 results. Each session had 10,000 indexed notes and zero waiting at both stopped-app and in-app boundaries. The long note's hash remained unchanged. All 100 launch-relative one-second samples are present; maximum scheduling deviation was 24 ms, and counter queries took 73–102 ms.

The delay removed the smaller startup-associated increase, but this experiment did **not** reproduce the original >1-second failure in either control. It therefore cannot establish that waiting fixes #159.

The controls overlap substantial startup activity: the app accumulates roughly 8 seconds of CPU time in its first five seconds. Another approximately 0.9 seconds of CPU and 5.7 MB of reads occur around seconds 7–9, consistent with the existing five-second-delayed semantic reconcile. By seconds 10–20 the app adds only 31–125 ms CPU and 39 KB of reads. Defender reads about 73.5 MB near launch and accumulates 2.9–3.3 seconds CPU through the measurement window. In B1, each of the five delayed scans adds approximately 45.9 MB of application reads: about 230 MB for the repeated traversal, despite an already settled index.

This supports startup resource contention, including Windows scanning, as a contributor. It does not identify the task responsible for the original failure. These counters report process CPU and logical I/O, not file attribution, physical disk misses, mutex wait, or per-query CPU. Cold OS-cache effects remain possible: sessions ran in fixed A1/A2/B1/B2 order, without purging the desktop's cache, and earlier setup attempts also warmed state. No Defender causal attribution or mutex diagnosis is established. The original failed perf row remains failed.

## Design recommendation

If implementing a structural repair now, prefer **(a), a private decoded-vector cache owned by `SemanticIndex`**, rather than a fixed user-visible delay. The measured repeated blob traversal is avoidable even when the machine is otherwise idle. The cache costs 30,793,728 bytes (about 31 MB, plus keys/container overhead) for 10,024 × 768 f32 values. SQLite and its pending queue remain durable; Markdown remains authoritative. No ANN dependency or schema migration is needed.

Load the current-profile vectors on a backend worker after open. Serialize loading, database mutation, and cache publication under the same existing ownership lock. Retain note identity and winning chunk ordinal; filter using current profile/category metadata. Keep cosine calculation, confidence threshold, best chunk per note, and score/title ordering unchanged. Fetch result text/path only after selection. Title ties crossing the limit require all tied candidates' titles before truncation; fetching only an arbitrary first 20 would change ranking.

For each successful upsert, publish cache changes only after the existing pending key/path/hash/profile check and SQLite commit. Evict every displaced identity, including a different key at the same path. Removal and reconcile deletion evict the corresponding keys after commit; rebuild clears the cache after its transaction; open/profile replacement reloads it. Failed transactions and obsolete embedding responses must leave the cache untouched. A first query must wait for a complete cache or receive an explicit initialization error, never silently search a partial cache. Record that wait: moving blob loading into an unmeasured first-query wait is not a fix.

**(b), narrower SQLite scans**, is the smaller implementation and avoids copying path/title/text for every chunk. It still reads/decodes roughly 31 MB of embeddings per query and may touch the same SQLite overflow pages, so it does not remove the measured I/O dependency. Retain winning chunk identity and resolve title ties before limiting. Starting the scan timer after acquiring the lock improves diagnosis only: also record lock wait and the original combined latency.

**(c), deferring/throttling reconcile**, may reduce contention, but a five-second head start already exists. The original first failure began earlier than that delayed reconcile; further delay alone cannot explain or repair every startup path. Throttling also extends convergence for offline edits/deletions. Preserve hash checks and eventual reconciliation; do not replace them with mtime guesses or a fixed 20-second readiness delay.

## Proof required for an implementation

Run `pnpm test:rust semantic_search::tests`. Add a deterministic comparison against the existing SQLite scorer covering multiple chunks, exact scores/snippets, category/profile filtering, cutoff and title ties at the limit. Exercise reopen, replace/move/delete/rebuild, a failed transaction, and an obsolete in-flight embedding response; verify both cache and durable database agree. Update the existing benchmark to populate the database before reopening/loading the cache, rather than bypassing cache maintenance with direct SQL after open.

Run the existing optimized 10,000-note benchmark through `pnpm test:rust --release semantic_search::tests::brute_force_search_latency_for_ten_thousand_notes -- --ignored --nocapture`, taking five samples without dropping the first. Instrument cache-load duration, mutex wait, post-lock scan/result hydration, and total retrieval; verify repeated scans no longer read all embedding blobs.

Finally rerun the installed protocol with **no pre-search delay** on Windows and Fedora, including the first query, with a settled index before every session. Require exact-scan p95 ≤250 ms, semantic-search p95 ≤2,000 ms, unchanged retrieval results, and existing startup budgets (median ≤3,000 ms, maximum ≤5,000 ms). Repeat launch/search on Windows, including a natural cold start, with the same counters. Measure RAM and cache readiness; do not hide startup work or lock wait by changing timer boundaries. Recheck a changed and removed note after reopening. The A/B experiment alone is not proof that the proposed cache fixes the original intermittent failure.

## Protocol and cleanup

The temporary probe copy adds the optional `PRE_SEARCH_DELAY_MS` at the search boundary. A log-based readiness prerequisite ensures both controls and delayed sessions begin after `startup-ready`; the inherited search-button selector can be visible earlier. The original failed perf run's first query started approximately 172 ms before its logged readiness, inferred from its embedding/scan durations. This timing difference is another limit on comparison with that historical run.

`perf-gate.ps1` remained the original interactive-launch path. An external standard-library WebDriver relay holds only session teardown until launch +26 seconds, after all five queries, keeping the app alive for every sample. Sampling uses the recorded launched driver's descendant PID and verifies executable/creation time. Defender is sampled through `Win32_PerfRawData_PerfProc_Process`, read only; it was never stopped. The user-owned Ollama service was used without lifecycle changes.

`discarded-attempts/` retains two setup attempts: the first relay port conflicted with the native driver before app launch; the second collected one session but exposed premature readiness and an unreliable PowerShell `Process.ExitCode` completion check. Neither contributes to the table. Their configuration restoration proofs are retained. Transcripts/profile journals redact workstation and profile identity; measurement rows are otherwise retained as emitted.

The original config was restored from `config.json.before-perf` and independently verified as SHA-256 `0BFB153904693EA54A471D3773889876B6CC0305FD7663489CAFC92AA20004B8`. Window-state hashes also match. All recorded app/driver PIDs are gone, both diagnostic scheduled tasks are removed, original profiles are restored, and the machine-state junction/configuration lock are absent. `final-proof.json` and `final-index.json` record the checks.

Run `python3 verify-evidence.py` from this directory to validate raw counts, readiness/delay boundaries, PID consistency, index settlement, unchanged note hash, and cleanup, and regenerate `summary.json`.
