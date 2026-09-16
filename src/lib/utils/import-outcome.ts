import type { BulkMutationTerminal, ImportResult } from "$lib/types";

/** What the backend emits on `import-done`: the shared terminal contract plus the counts. */
export type ImportDonePayload = BulkMutationTerminal & Partial<ImportResult>;

export interface ImportOutcomeView {
  /** Counts to show, or null when the run changed nothing. */
  result: ImportResult | null;
  /** Reason to show, or null when the run succeeded outright. */
  error: string | null;
  /**
   * Whether the workspace needs refreshing. The watcher is suppressed for the duration of a
   * bulk mutation, so notes the import rewrote are invisible until something re-reads them.
   */
  refresh: boolean;
}

/**
 * Decide what a finished import shows.
 *
 * A `changed-incomplete` run converted some notes and failed on others, so it reports both
 * halves: showing only the reason would hide what changed, and showing only the counts would
 * hide that the vault is half-converted.
 */
export function importOutcomeView(data: ImportDonePayload): ImportOutcomeView {
  const changed = data.outcome !== "failure";
  return {
    result: changed
      ? {
          files_converted: data.files_converted ?? 0,
          links_converted: data.links_converted ?? 0,
          frontmatter_normalized: data.frontmatter_normalized ?? 0,
          syntax_converted: data.syntax_converted ?? 0,
          attachments_moved: data.attachments_moved ?? 0,
        }
      : null,
    error: data.success ? null : (data.error ?? "Import failed"),
    refresh: changed,
  };
}
