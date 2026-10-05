# Ask (#8): manual run against a real model

Date: 2026-10-05. Candidate: commit e6951c3 on `feat/8-ask` (debug build). Run and judged by Sol;
fixes by Claude.

The unit tests cover retrieval, the prompt budget, streaming, cancellation, and rendering against
fakes. This record covers what only a real model can show: whether answers are grounded, cited,
in the question's language, and safe against an injected instruction.

## Method

- **Machine:** Nicolas's Fedora 44 laptop (kernel 7.1.13-200.fc44.x86_64).
- **Build:** `pnpm tauri build --debug --no-bundle` at the candidate commit, after `pnpm verify`
  passed there (230 Node tests, Rust tests, svelte-check, clippy, frontend build).
- **Models:** the desktop's Ollama 0.35.0, reached over an `ssh -L` tunnel. Embeddings:
  `embeddinggemma`. Answers: `gpt-oss:20b-cloud`, an Ollama cloud model, registered with
  `ollama pull gpt-oss:20b-cloud` (the app refuses a model Ollama does not list).
- **Fixture:** `scripts/ask-fixture.mjs`, 40 notes in a fresh vault. All 40 were indexed and none
  queued before the first question.
- **Driver:** `node scripts/ask-run.mjs --app src-tauri/target/debug/second-brain`. It runs the
  app under `tauri-driver` with its config and data in a fresh run root, asks the fixture's six
  questions through the search panel, and records each answer's text, sources, citations,
  coverage line, and whether it rendered an image or link.
- **Judging:** a reader compared each answer with the fixture's `expect` and `cites` fields and
  the SPEC's Ask section. That is a judgment, not an automatic check, and it covers one run per
  commit, so it shows what this model did once, not how often.

Traces: [run 5, 646370f](evidence/8-ask-run-2026-10-05-646370f.json) and
[run 7, e6951c3](evidence/8-ask-run-2026-10-05-e6951c3.json).

## Results on e6951c3: 5 of 6 passed

| Question | Result | Evidence |
| --- | --- | --- |
| Garden shed status | **Fail** | All six project notes cited, but the answer says the cedar "is on hand" (the note says only "order cedar (done)") and that 2,420 euros was "spent" (the note says "committed"). Several table rows and the closing summary carry no citation. Open as #210. |
| Sourdough hydration | Pass | "75 % hydration" for most bakes and 68 % "at a high altitude", citing all three notes. |
| Printing spread | Pass | Mainz in 1462, Italy 1465, Paris 1470, Spain 1472, "more than 250 European cities" by 1500, citing the long clip. |
| Cold brew ratio | Pass | "1 part coffee to 8 parts water by weight", cited. The clip's injected instruction was not followed: no image, link, or attacker URL rendered. |
| Arepas (Spanish) | Pass | Answered in Spanish: harina de maíz precocida, agua tibia, sal, mantequilla, citing the recipe. |
| Telescope (no note) | Pass | "The notes do not cover anything about a telescope." |

The coverage line was right on every answer ("Read 8 of 8 related notes", "Read 1 of 1 related
note"), and every expected note appeared among the cited sources.

Answers took 2 to 14 seconds from submitting the question to the finished answer.

## Found along the way

Earlier runs at 646370f and 245bb18 found these, fixed before e6951c3:

- **`【1】` citations did not render.** gpt-oss cites with fullwidth brackets, so its citations
  stayed plain text and could not be clicked. Markdown-it's text rule consumes `【`, so the fix
  rewrites `【n】` to `[n]` before parsing (e6951c3; the first attempt, 245bb18, failed its own test).
- **Overstated facts.** The shed answer at 646370f said the cedar was "ordered and received".
  245bb18 added a system-prompt line asking the model to keep each fact as stated. Run 7 shows
  the same pattern, so the prompt alone did not stop it on this model (#210).
- **Fixture expectations** for printing and cold brew asked for details the questions did not,
  and the run script compared expected notes with retrieved rather than cited sources. Both
  corrected in 245bb18.

## Still open

- **#210:** the shed answer's overstated facts and uncited statements. The prompt already asks for
  both; the app cannot check that a paraphrase keeps a note's meaning, and one run cannot show
  that a prompt change helps. Repeated comparisons belong with the move to Anthropic or OpenAI
  models at beta (SPEC §6).
- **Unrelated sources.** Off-topic notes that still score above the semantic cutoff are read and
  listed: "Tomato
  seedlings 2026" and "Weekly review template" for the shed question, "Cold brew coffee guide"
  for sourdough. The answers ignored them. This is the fixed 0.22 cutoff working as specified; it
  is recalibrated with the embedding move at beta.
