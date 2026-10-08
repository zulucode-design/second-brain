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

/** The overlay's two fields (#213). Both are required. */
export type CaptureField = 'title' | 'body';

export interface CaptureKey {
	key: string;
	ctrlKey?: boolean;
	metaKey?: boolean;
	shiftKey?: boolean;
	/** True while an input method is composing; its Enter confirms the composition. */
	isComposing?: boolean;
}

export interface CaptureState {
	title: string;
	body: string;
	/** The field with the caret. */
	field: CaptureField;
	selected: number;
}

export type CaptureAction =
	| { type: 'none' }
	/** Let the field handle it. Anything that inserts a character lands here. */
	| { type: 'insert' }
	| { type: 'focus'; field: CaptureField }
	| { type: 'choose'; phase: 'choosing' }
	/** The picker was asked for with a field still empty: say which, and put the caret there. */
	| { type: 'missing'; field: CaptureField }
	| { type: 'move'; delta: number }
	/** File it. `category` is the one under the cursor, or the digit that was pressed. */
	| { type: 'save'; category: CaptureCategory }
	| { type: 'back' }
	| { type: 'dismiss' }
	| { type: 'confirmDiscard' };

/** The first empty field, title before body, or null when both are filled. */
export function missingField(title: string, body: string): CaptureField | null {
	if (!title.trim()) return 'title';
	if (!body.trim()) return 'body';
	return null;
}

export const MISSING_FIELD_MESSAGE: Record<CaptureField, string> = {
	title: 'Add a title.',
	body: 'Add a body.'
};

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
	state: CaptureState
): CaptureAction {
	const accelerator = event.ctrlKey || event.metaKey;

	if (phase === 'writing') {
		if (event.isComposing) return { type: 'insert' };
		// Esc on an empty overlay just closes it. With text in either field, throwing it away
		// silently is the one unrecoverable thing this window can do, so it asks first.
		if (event.key === 'Escape') {
			return state.title.trim() || state.body.trim()
				? { type: 'confirmDiscard' }
				: { type: 'dismiss' };
		}
		// Ctrl+Enter is the only way to the picker, so a Tab meant for the other field never
		// opens it by accident.
		if (event.key === 'Enter' && accelerator) {
			const missing = missingField(state.title, state.body);
			return missing ? { type: 'missing', field: missing } : { type: 'choose', phase: 'choosing' };
		}
		// Two fields, so Tab and Shift+Tab both land on the other one and wrap.
		if (event.key === 'Tab') {
			return { type: 'focus', field: state.field === 'title' ? 'body' : 'title' };
		}
		// A title is one line, so Enter there moves on. In the body it is a newline: a capture is
		// often more than one line.
		if (event.key === 'Enter' && state.field === 'title') {
			return { type: 'focus', field: 'body' };
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
