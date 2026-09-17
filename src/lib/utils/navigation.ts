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
/** The app's own saves are not echoed by the vault watcher, so views that list note content
 * listen for this instead. `detail` is `{ path, hasTasks }`. */
export const NOTE_SAVED_EVENT = 'helix:note-saved';
