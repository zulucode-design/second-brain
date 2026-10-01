import type { Editor, Content } from '@tiptap/core';
import type { Transaction } from '@tiptap/pm/state';

/**
 * Whether a TipTap `update` event is an edit the user made (#196). Only a document change
 * counts: setEditable() emits an update with an unchanged transaction. Nothing counts while a
 * note is loading.
 */
export function isEditUpdate(
	event: { transaction: Transaction; appendedTransactions?: Transaction[] },
	loading: boolean,
): boolean {
	if (loading) return false;
	return event.transaction.docChanged || (event.appendedTransactions ?? []).some((tr) => tr.docChanged);
}

/**
 * Replaces the document without an update event: a load is not an edit. A one-shot "ignore the
 * next update" flag stayed armed when the content was identical and TipTap emitted nothing, and
 * then swallowed the user's first real edit.
 */
export function loadContent(editor: Editor, content: Content): void {
	editor.commands.setContent(content, { emitUpdate: false });
}
