# ADR-0007: Use EmbeddingGemma with versioned local chunk embeddings

- **Status**: Accepted
- **Date**: 2026-09-10
- **Context**: Ticket #7, note embeddings and semantic search

## Context

Semantic search needs one embedding space for both stored note chunks and each query. The
choice must work through the project's existing Ollama connection, remain practical on the
desktop AI host, handle the user's multilingual notes, and have an explicit migration path.
Markdown remains the source of truth, so the vector store must be disposable.

The existing Tantivy keyword index already works and remains independent. This decision does
not consolidate keyword and semantic retrieval or make literal search depend on Ollama.

## Decision

Use Ollama's `embeddinggemma` model through `POST /api/embed`. Ollama describes it as a
300-million-parameter model designed for retrieval and trained across more than 100 spoken
languages. The current published model is 622 MB, emits 768-dimensional vectors, has a 2K
context window, and requires Ollama 0.11.10 or later. See the
[Ollama model page](https://ollama.com/library/embeddinggemma),
[model metadata](https://ollama.com/library/embeddinggemma%3A300m/blobs/0800cbac9c20), and
[embed API](https://docs.ollama.com/api/embed).

Semantic retrieval has a fixed v1 profile:

```text
ollama:embeddinggemma:chunks-v1
```

The profile covers all compatibility-sensitive inputs, not only the displayed model name.
Each note is split into overlapping chunks of at most 1,500 Unicode characters with a
200-character overlap. Its title prefixes every embedding input. `truncate` is disabled in
the Ollama request so the backend cannot silently embed less text than the stored result
snippet claims. Each note contributes only its best-scoring chunk to the final result list.

Vectors, chunk text, note identity/path metadata, and a durable pending-work queue live in a
machine-local SQLite database. Note changes enqueue local work synchronously; a background
worker owns network inference and retries while Ollama is unavailable. Stable note IDs are
used when present, with the current path retained for opening and deletion. Legacy notes
without IDs use their path as a fallback identity.

When the profile changes, existing vectors remain physically harmless but are not searchable.
Vault reconciliation sees their profile mismatch and queues the current Markdown note for a
fresh embedding. The new vectors replace the old note transactionally. A corrupt SQLite file
is replaced only when SQLite identifies it as corrupt or not a database, after which Markdown
is reconciled back into the empty projection.

## Retrieval decision and measurement

Use exact brute-force cosine similarity over the SQLite blobs for v1. The committed ignored
benchmark creates 10,000 notes with one 768-dimensional vector each and runs the production
decode, cosine, best-per-note, sort, and limit path.

On 2026-09-10, an optimized Rust test build on Linux 7.1.13 with an Intel Core i5-9300H
(4 cores / 8 threads) measured **119 ms** for a 10,000-note scan (the same run shape has
varied between 60–119 ms as the machine was loaded). The matching debug build took 1,894 ms
and is not representative of the shipped binary. Reproduce the release measurement:

```sh
pnpm test:rust --release semantic_search::tests::brute_force_search_latency_for_ten_thousand_notes -- --ignored --nocapture
```

Sixty milliseconds for a deliberately upper-end personal vault does not justify introducing
an approximate-nearest-neighbour dependency, index format, or synchronization path. Revisit
this only if measurements on real larger vaults make local vector scanning a user-visible
bottleneck.

## Consequences

- Semantic search requires `embeddinggemma` on the configured Ollama host, even when writing
  tools use another provider. This keeps the knowledge index stable when writing providers
  change.
- A sleeping or unreachable host delays embeddings but never note saves or keyword search.
- Long notes are searchable beyond their opening text and can create multiple stored vectors.
- Changing the model, its vector behavior, or chunking requires a new profile string. No
  in-place comparison of incompatible vectors is allowed.
- SQLite is derived machine-local data; deleting it loses no note content.

## Alternatives rejected

| Option | Why not |
| --- | --- |
| Reuse the selected writing model/provider | Provider changes would invalidate the whole knowledge index and some providers do not expose compatible embeddings. |
| Embed one vector per complete note | Long notes exceed model context and a single vector hides locally relevant passages. |
| Silently truncate at the backend | The result snippet could claim text that the vector never represented. |
| Add HNSW, `sqlite-vec`, or another ANN index now | The measured 60 ms exact scan at 10,000 notes is already responsive and exact; ANN adds a second index lifecycle without evidence that it is needed. |
| Replace Tantivy with SQLite | Literal search is working, tested, and must remain available without AI. |
