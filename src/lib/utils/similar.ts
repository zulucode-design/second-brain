import type { SimilarityCheck } from '$lib/types';

/** One similarity check shown in the main window (#9). */
export interface SimilarCard {
  id: string;
  /** The vault the checked note lives in; a vault switch drops every other card. */
  vault: string;
  check: SimilarityCheck;
  busy: boolean;
  error: string | null;
}

/** Newest first. A newer check of the same note replaces its older card. */
export function withCard(cards: SimilarCard[], card: SimilarCard): SimilarCard[] {
  return [card, ...cards.filter((item) => item.check.capture.path !== card.check.capture.path)];
}

export function forVault(cards: SimilarCard[], vault: string | null | undefined): SimilarCard[] {
  return cards.filter((card) => card.vault === vault);
}

/** "Showing 3 of 5 similar notes" when more passed the bar than the card shows. */
export function similarCoverage(check: SimilarityCheck): string | null {
  return check.total > check.matches.length
    ? `Showing ${check.matches.length} of ${check.total} similar notes`
    : null;
}

interface OpenNote {
  id: string;
  path: string;
}

/**
 * Notes created in the app this session, each checked once: the first time the user leaves it,
 * when the thought is written rather than half-typed. Reopening an older note never checks it.
 */
export function createLeaveWatcher() {
  const created = new Set<string>();
  let current: OpenNote | null = null;
  return {
    created(id: string) {
      created.add(id);
    },
    /** The open note became `note`; returns the path of a new note just left, to check. */
    opened(note: OpenNote | null): string | null {
      const left = current;
      current = note;
      // The same note under a new path is a rename, not leaving it.
      if (!left || left.id === note?.id || !created.delete(left.id)) return null;
      return left.path;
    },
  };
}
