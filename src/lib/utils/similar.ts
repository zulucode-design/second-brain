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

/** What an append did. 'capture-kept': the note was written but the capture stayed. null:
 * nothing was written. */
export type AppendOutcome = 'appended' | 'capture-kept' | null;

/**
 * Runs an append with the open note held, when it is one of the two: saved first, then kept
 * read-only until `settle` has shown the result, so nothing typed meanwhile is lost with a
 * trashed capture or under the reloaded note.
 */
export async function appendWhileHeld(steps: {
  held: boolean;
  /** Makes the editor read-only; the returned function lets it go. */
  hold: () => Promise<() => void>;
  save: () => Promise<boolean>;
  append: () => Promise<AppendOutcome>;
  settle: (outcome: 'appended' | 'capture-kept') => void;
}): Promise<void> {
  const release = steps.held ? await steps.hold() : null;
  try {
    if (release && !(await steps.save())) return;
    const outcome = await steps.append();
    if (outcome) steps.settle(outcome);
  } finally {
    release?.();
  }
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
