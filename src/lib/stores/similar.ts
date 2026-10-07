import { getCurrentWindow } from '@tauri-apps/api/window';
import { get, writable } from 'svelte/store';
import { appendToSimilarNote, checkSimilarNotes } from '$lib/api';
import { listenAppEvent } from '$lib/events';
import { activeNote, appConfig } from '$lib/stores/app';
import type { CheckedNote, SimilarityCheck } from '$lib/types';
import { createLeaveWatcher, forVault, withCard, type AppendOutcome, type SimilarCard } from '$lib/utils/similar';

/** Similarity cards, newest first. Held in memory only, so quitting drops them. */
export const similarCards = writable<SimilarCard[]>([]);

function show(check: SimilarityCheck) {
  const vault = get(appConfig)?.active_vault;
  // A check that finished after a vault switch belongs to the vault that was left.
  if (!vault || check.vault !== vault) return;
  similarCards.update((cards) =>
    withCard(cards, { id: crypto.randomUUID(), vault, check, busy: false, error: null }),
  );
}

function patch(id: string, change: Partial<SimilarCard>) {
  similarCards.update((cards) => cards.map((card) => (card.id === id ? { ...card, ...change } : card)));
}

/** The card's action failed; the card stays and says why. */
export function similarCardFailed(id: string, error: string) {
  patch(id, { busy: false, error });
}

export function dismissSimilar(id: string) {
  similarCards.update((cards) => cards.filter((card) => card.id !== id));
}

/**
 * Folds the card's capture into `target`. 'appended' once done and the card is gone;
 * 'capture-kept' when the note was written but the capture could not be moved to trash, which
 * the card then says; null when nothing was written.
 */
export async function appendSimilar(
  id: string,
  capture: CheckedNote,
  target: CheckedNote,
): Promise<AppendOutcome> {
  patch(id, { busy: true, error: null });
  try {
    const trashFailure = await appendToSimilarNote(capture, target);
    if (trashFailure) {
      patch(id, { busy: false, error: trashFailure });
      return 'capture-kept';
    }
    dismissSimilar(id);
    return 'appended';
  } catch (error) {
    patch(id, { busy: false, error: String(error) });
    return null;
  }
}

const watcher = createLeaveWatcher();

/** A note was created in the app; it is checked when the user first leaves it. Only the main
 * window shows cards, so a note made in a note window is not checked. */
export function noteCreatedInApp(id: string) {
  if (getCurrentWindow().label === 'main') watcher.created(id);
}

// Quick captures are checked by the backend, which sends what it found.
void listenAppEvent('similarNotesFound', ({ payload }) => show(payload));

activeNote.subscribe((note) => {
  const left = watcher.opened(note ? { id: note.meta.id, path: note.path } : null);
  if (!left) return;
  checkSimilarNotes(left)
    .then((check) => check && show(check))
    .catch((error) => console.error('Similarity check failed:', error));
});

appConfig.subscribe((config) => {
  similarCards.update((cards) => forVault(cards, config?.active_vault));
});
