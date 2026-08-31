import type { ViewMode } from '$lib/types';

export interface NoteRowPolicy {
	holdingPreview: boolean;
	fileUnder: boolean;
	rename: boolean;
	drag: boolean;
	contextMenu: boolean;
	quickAccess: boolean;
}

export function noteRowPolicy(viewMode: ViewMode): NoteRowPolicy {
	const holding = viewMode === 'unfiled';
	return {
		holdingPreview: holding,
		fileUnder: holding,
		rename: !holding,
		drag: !holding,
		contextMenu: !holding,
		quickAccess: !holding
	};
}
