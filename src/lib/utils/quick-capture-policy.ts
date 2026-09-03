/**
 * What each key does in the quick-capture overlay.
 *
 * Kept out of the component because this is the part that has to be right: the overlay is
 * keyboard-only by design, so a key that does the wrong thing is the whole feature failing.
 * It is also the part with no visible state to inspect afterwards — the window is gone.
 */

export const CAPTURE_CATEGORIES = ['Projects', 'Areas', 'Resources', 'Archives'] as const;

export type CaptureCategory = (typeof CAPTURE_CATEGORIES)[number];

/** Typing what you came to write, then choosing where it goes. */
export type CapturePhase = 'writing' | 'choosing';

export interface CaptureKey {
	key: string;
	ctrlKey?: boolean;
	metaKey?: boolean;
	shiftKey?: boolean;
}

export type CaptureAction =
	| { type: 'none' }
	/** Let the textarea handle it. Anything that inserts a character lands here. */
	| { type: 'insert' }
	| { type: 'choose'; phase: 'choosing' }
	| { type: 'move'; delta: number }
	/** File it. `category` is the one under the cursor, or the digit that was pressed. */
	| { type: 'save'; category: CaptureCategory }
	| { type: 'back' }
	| { type: 'dismiss' }
	| { type: 'confirmDiscard' };

/**
 * A digit is a direct accelerator only while choosing.
 *
 * While writing, "1" is a character the user meant to type. Treating it as a category there
 * would eat digits out of captured text, which is both wrong and invisible until later.
 */
function categoryForDigit(key: string): CaptureCategory | null {
	const index = Number(key) - 1;
	return Number.isInteger(index) && index >= 0 && index < CAPTURE_CATEGORIES.length
		? CAPTURE_CATEGORIES[index]
		: null;
}

export function captureAction(
	phase: CapturePhase,
	event: CaptureKey,
	state: { hasText: boolean; selected: number }
): CaptureAction {
	const accelerator = event.ctrlKey || event.metaKey;

	if (phase === 'writing') {
		// Esc on an empty overlay just closes it. With text in it, throwing the text away
		// silently is the one unrecoverable thing this window can do, so it asks first.
		if (event.key === 'Escape') {
			return state.hasText ? { type: 'confirmDiscard' } : { type: 'dismiss' };
		}
		// Enter is a newline: a capture is often more than one line, and the alternative
		// costs the user a paste that ends mid-thought.
		if (event.key === 'Enter' && accelerator) {
			return state.hasText ? { type: 'choose', phase: 'choosing' } : { type: 'none' };
		}
		if (event.key === 'Tab') {
			return state.hasText ? { type: 'choose', phase: 'choosing' } : { type: 'none' };
		}
		return { type: 'insert' };
	}

	if (event.key === 'Escape') {
		// Back to the text, not gone: the user is still mid-capture and has typed something
		// worth keeping.
		return { type: 'back' };
	}
	const digit = categoryForDigit(event.key);
	if (digit && !accelerator) {
		return { type: 'save', category: digit };
	}
	if (event.key === 'ArrowRight' || event.key === 'ArrowDown') {
		return { type: 'move', delta: 1 };
	}
	if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
		return { type: 'move', delta: -1 };
	}
	if (event.key === 'Enter') {
		return { type: 'save', category: CAPTURE_CATEGORIES[state.selected] };
	}
	return { type: 'none' };
}

/** Wraps, so holding one arrow key always reaches every category. */
export function moveSelection(selected: number, delta: number): number {
	const count = CAPTURE_CATEGORIES.length;
	return (selected + delta + count) % count;
}
