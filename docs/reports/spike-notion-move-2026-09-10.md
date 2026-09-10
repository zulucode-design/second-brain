# Notion move-endpoint spike (#12) — 2026-09-10

## Scope

The design for #12 rests on one assumption the API docs do not guarantee: that
`POST /v1/pages/{page_id}/move` relocates a page to another data source **without
changing its identity**. If it does not, "moving a note between categories preserves
identity" is unachievable, and the note-to-page mapping, the tombstone scheme, and the
four-database layout would all need redesigning around delete-and-recreate.

This spike answers that question against a live disposable workspace before any of that
is built on top of it.

Run twice, same results both times. Not run against production data: a throwaway workspace
was created for it, and both spike databases are trashed on exit including on failure.

## Findings

| Finding | Evidence | Confidence |
| --- | --- | --- |
| The page id is unchanged by a move between data sources. | Page `3d7c83f7-…-812b-…` was identical before and after `POST /v1/pages/{id}/move`. | High |
| The move reparents to the target data source. | `parent.data_source_id` after the move equals the Areas data source id. | High |
| A move is a move, not a copy. | Querying the source data source afterwards returns zero results. | High |
| Content blocks survive. | `heading_1,paragraph,code` present in the same order before and after. | High |
| `title` and `rich_text` properties survive. | Title `Spike note` and `Note ID` `spike-note-0001` both intact after the move. | High |
| **`multi_select` values are dropped by the move.** | `Tags` held `[{name:"spike"}]` before and `[]` after, with schemas identical by name and type in both data sources. | High |
| A follow-up `PATCH /v1/pages/{id}` restores the dropped values, and does not change the page id. | `Tags` read back as `spike` after the rewrite; page id unchanged. | High |
| The map is rebuildable from Notion alone. | Querying the target data source filtered on `Note ID` returned exactly the moved page. | High |
| Creating a database returns its initial data source id. | `data_sources[0].id` present in the create response for both databases. | High |

## Why multi_select drops

Property ids are per data source: `Tags` was `kf%3EC` in Projects and `%3EiRC` in Areas.
`multi_select` values reference **options registered on the property**, and those options
belong to the data source that owns it. The Areas data source had never seen an option
named `spike`, so the value had nowhere to land and was dropped.

Title and rich_text survive precisely because they carry no option registry — their value
is self-contained.

This will therefore also affect `select`, `status`, and `relation`, which are
registry-backed for the same reason. Of these, only `multi_select` is in the schema #12
settled on (`Tags`), but the rule matters for any property added later.

## Consequence for #12

**A category move is two calls, not one: move, then rewrite properties.** The rewrite is
not conditional on the note having changed — the move alone loses tag data, so the write
must follow every move.

The alternative — pre-registering every tag option in all four data sources so values
always have a home — was rejected: tags are open-ended and grow over time, so it converts
a bounded per-move cost into an unbounded schema-maintenance problem, and it still fails
the first time a note carries a tag created since the last schema sync. Rewriting after
the move is self-healing and costs one request against a 3 req/s budget, on an operation
that is rare by nature.

## What this does not cover

- Moves between data sources whose schemas **differ**. Not tested, because production
  gives all four databases the same schema. If that ever stops being true, retest.
- Pacing under a large first sync, and the guided setup flow end to end. Both still
  require live verification before #12 closes; this spike deliberately covered only the
  assumption that would have invalidated the design.

## Reproduction

The spike script is deliberately not committed — it is throwaway verification holding a
workspace-scoped token path, and it creates and destroys databases. It lived at
`~/second-brain-notion-spike.sh` during this work, reading its token from
`~/.config/second-brain-notion-test-token` and never printing it.
