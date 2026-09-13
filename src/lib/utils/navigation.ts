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