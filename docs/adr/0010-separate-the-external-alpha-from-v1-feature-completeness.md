# Separate the external alpha from v1 feature completeness

- **Status**: Accepted
- **Date**: 2026-09-13

The external alpha validates a safe Capture-and-Organize desktop core rather than every v1
feature. It includes keyword and semantic search, using semantic retrieval as the public and
testable seam for later AI features. The prompt/Q&A window (#8), capture-time similarity
(#9), semantic graph edges and link promotion (#10), and voice memo capture/transcription
(#11) remain v1 commitments but move to the named **v1 feature-complete beta** milestone.

This boundary gets real users onto the persistence, PARA, retrieval, backup, sync, and
graceful-degradation paths before adding four substantial interaction surfaces. It does not
turn those features into an indefinite backlog: their existing tickets remain owned by the
project maintainer and block v1 feature-complete beta, not external alpha. Product copy and
release evidence must distinguish available external-alpha behavior from beta commitments.
