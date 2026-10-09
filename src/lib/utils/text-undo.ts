/**
 * Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y for plain text fields.
 *
 * The Linux WebView keeps an undo history for inputs and textareas but binds no key to it, so
 * Ctrl+Z does nothing there (#213). `document.execCommand` reaches the same history. The note
 * editor is not a plain field and keeps its own undo.
 */

export interface UndoKey {
	key: string;
	ctrlKey?: boolean;
	metaKey?: boolean;
	shiftKey?: boolean;
	altKey?: boolean;
}

export function undoCommand(event: UndoKey): 'undo' | 'redo' | null {
	if (!(event.ctrlKey || event.metaKey) || event.altKey) return null;
	const key = event.key.toLowerCase();
	if (key === 'z') return event.shiftKey ? 'redo' : 'undo';
	if (key === 'y' && !event.shiftKey) return 'redo';
	return null;
}
