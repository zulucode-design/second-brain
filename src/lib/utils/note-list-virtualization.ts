import type { ViewMode } from '$lib/types';

interface NoteListWindowInput {
	viewMode: ViewMode;
	itemCount: number;
	itemHeight: number;
	scrollTop: number;
	containerHeight: number;
	buffer?: number;
}

export interface NoteListWindow {
	startIndex: number;
	endIndex: number;
	topPad: number;
	bottomPad: number;
}

export function noteListWindow({
	viewMode,
	itemCount,
	itemHeight,
	scrollTop,
	containerHeight,
	buffer = 10
}: NoteListWindowInput): NoteListWindow {
	if (viewMode === 'unfiled') {
		return { startIndex: 0, endIndex: itemCount, topPad: 0, bottomPad: 0 };
	}

	const startIndex = Math.max(0, Math.floor(scrollTop / itemHeight) - buffer);
	const endIndex = Math.min(
		itemCount,
		Math.ceil((scrollTop + containerHeight) / itemHeight) + buffer
	);
	return {
		startIndex,
		endIndex,
		topPad: startIndex * itemHeight,
		bottomPad: Math.max(0, (itemCount - endIndex) * itemHeight)
	};
}
