export const NAVIGATE_NOTE_EVENT = 'helix:navigate-note';

export interface NavigateNoteRequest {
	path: string;
}

export function requestNoteNavigation(path: string): void {
	window.dispatchEvent(new CustomEvent<NavigateNoteRequest>(NAVIGATE_NOTE_EVENT, {
		detail: { path },
	}));
}


export type NoteNavigationResult = 'navigated' | 'save-failed' | 'not-found' | 'blocked';

/**
 * Whether a note read for navigation is the one the caller meant. Navigation can wait on a
 * save before it reads, and a sync can put a different note at the path meanwhile; with no
 * expected id, any note at the path will do.
 */
export function isExpectedNote(note: { meta: { id: string } }, expectedId?: string | null): boolean {
	return !expectedId || note.meta.id === expectedId;
}
/** The app's own saves are not echoed by the vault watcher, so views that list note content
 * listen for this instead. `detail` is `{ path, hasTasks }`. */
export const NOTE_SAVED_EVENT = 'helix:note-saved';
