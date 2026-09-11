# Domain context

Second Brain is a local-first knowledge app. Markdown notes in one vault are the source of
truth; indexes and queues are machine-local projections that may be deleted and rebuilt.

## Glossary

| Term | Meaning |
| --- | --- |
| **Vault** | The one Markdown collection containing all four PARA categories. |
| **Keyword index** | The existing Tantivy projection used for literal full-text search. It does not depend on AI. |
| **Semantic index** | The SQLite projection of chunk embeddings used to search by meaning. It is separate from the keyword index. |
| **Embedding profile** | The versioned contract that names the embedding backend, model, and chunking scheme. Vectors from different profiles are never compared. |
| **Pending embedding** | Durable machine-local work recording that a current Markdown note still needs vectors. Saving a note succeeds even when inference is unavailable. |

Use **semantic search** for retrieval by meaning. Do not use “AI search” as a synonym: the
writing tools and their selectable provider are a separate capability.
