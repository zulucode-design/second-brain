import { WebviewWindow, getAllWebviewWindows } from '@tauri-apps/api/webviewWindow';
import { get } from 'svelte/store';
import { shutdownPending } from '$lib/stores/app';
import { cancelNoteWindowReservation, reserveNoteWindow } from '$lib/api';

const openNoteWindows = new Map<string, string>();

export async function openNoteWindow(notePath: string, noteTitle: string) {
	if (get(shutdownPending)) throw new Error('Cannot open a note window while closing');
	const existingLabel = openNoteWindows.get(notePath);
	if (existingLabel) {
		const existing = await WebviewWindow.getByLabel(existingLabel);
		if (get(shutdownPending)) throw new Error('Cannot open a note window while closing');
		if (existing) {
			await existing.show();
			await existing.unminimize();
			await existing.setFocus();
			return;
		}
		openNoteWindows.delete(notePath);
	}

	// Native reservation and exit/vault-switch admission happen under the same lock.
	const reservation = await reserveNoteWindow();
	if (get(shutdownPending)) {
		await cancelNoteWindowReservation(reservation.label, reservation.token);
		throw new Error('Cannot open a note window while closing');
	}
	const url = `/?note=${encodeURIComponent(notePath)}&reservation=${encodeURIComponent(reservation.token)}`;
	let created = false;
	try {
		const noteWindow = new WebviewWindow(reservation.label, {
			url,
			title: `${noteTitle} - HelixNotes`,
			width: 900,
			height: 700,
			minWidth: 500,
			minHeight: 400,
			decorations: false,
			center: true
		});
		created = true;
		openNoteWindows.set(notePath, reservation.label);
		noteWindow.once('tauri://destroyed', () => openNoteWindows.delete(notePath));
		noteWindow.once('tauri://error', (e) => {
			console.error('Failed to create note window:', e);
			openNoteWindows.delete(notePath);
			void cancelNoteWindowReservation(reservation.label, reservation.token);
		});
	} finally {
		if (!created) await cancelNoteWindowReservation(reservation.label, reservation.token);
	}
}


/** Close every secondary through its native save-before-close handshake and verify destruction. */
export async function closeSecondaryWindowsForVaultSwitch(timeoutMs = 10_000): Promise<boolean> {
	const windows = (await getAllWebviewWindows()).filter((window) => window.label.startsWith('note-'));
	for (const window of windows) {
		await window.close();
		const deadline = Date.now() + timeoutMs;
		while (Date.now() < deadline) {
			if ((await WebviewWindow.getByLabel(window.label)) === null) break;
			await new Promise((resolve) => setTimeout(resolve, 50));
		}
		if ((await WebviewWindow.getByLabel(window.label)) !== null) return false;
	}
	return true;
}
