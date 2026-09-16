# Domain context

Second Brain is a local-first knowledge app. Markdown notes in one vault are the source of
truth; indexes and queues are machine-local projections that may be deleted and rebuilt.

## Glossary

| Term | Meaning |
| --- | --- |
| **Vault** | The one Markdown collection containing all four PARA categories. |
| **Logical vault** | The single Second Brain collection as a whole, regardless of which supported machine holds a replica. |
| **Vault replica** | One machine's local filesystem copy of the logical vault. Windows and Fedora replicas synchronize but remain locally usable. |
| **Keyword index** | The existing Tantivy projection used for literal full-text search. It does not depend on AI. |
| **Semantic index** | The SQLite projection of chunk embeddings used to search by meaning. It is separate from the keyword index. |
| **Embedding profile** | The versioned contract that names the embedding backend, model, and chunking scheme. Vectors from different profiles are never compared. |
| **Semantic schema version** | The versioned contract for the *shape* of the semantic index file, recorded in it and checked when it is opened. A file this build cannot interpret is rebuilt from Markdown rather than reused. Separate from the embedding profile: this describes the store, that describes what its vectors mean. |
| **Pending embedding** | Durable machine-local work recording that a current Markdown note still needs vectors. Saving a note succeeds even when inference is unavailable. |
| **External note** | A Markdown file outside the vault that the user opened explicitly. It is shown read-only in the external viewer and never saved, moved, or deleted by the app. |

Use **semantic search** for retrieval by meaning. Do not use “AI search” as a synonym: the
writing tools and their selectable provider are a separate capability.
