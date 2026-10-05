import { get, writable } from 'svelte/store';
import { appendToSimilarNote, checkSimilarNotes } from '$lib/api';
import { listenAppEvent } from '$lib/events';
import { activeNote, appConfig } from '$lib/stores/app';
import type { CheckedNote, SimilarityCheck } from '$lib/types';
import { createLeaveWatcher, forVault, withCard, type SimilarCard } from '$lib/utils/similar';

/** Similarity cards, newest first. Held in memory only, so quitting drops them. */
export const similarCards = writable<SimilarCard[]>([]);

function show(check: SimilarityCheck) {
  const vault = get(appConfig)?.active_vault;
  if (!vault) return;
  similarCards.update((cards) =>
    withCard(cards, { id: crypto.randomUUID(), vault, check, busy: false, error: null }),
  );
}

function patch(id: string, change: Partial<SimilarCard>) {
  similarCards.update((cards) => cards.map((card) => (card.id === id ? { ...card, ...change } : card)));
}

export function dismissSimilar(id: string) {
  similarCards.update((cards) => cards.filter((card) => card.id !== id));
}

/** Folds the card's capture into `target`; true once appended and the card is gone. */
export async function appendSimilar(id: string, capture: CheckedNote, target: CheckedNote): Promise<boolean> {
  patch(id, { busy: true, error: null });
  try {
    await appendToSimilarNote(capture, target);
    dismissSimilar(id);
    return true;
  } catch (error) {
    patch(id, { busy: false, error: String(error) });
    return false;
  }
}

const watcher = createLeaveWatcher();

/** A note was created in the app; it is checked when the user first leaves it. */
export function noteCreatedInApp(id: string) {
  watcher.created(id);
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
