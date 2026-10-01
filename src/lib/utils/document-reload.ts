export interface ReloadCapture {
	path: string;
	revision: string;
}

export interface CleanDocumentReloadOptions<T extends { revision: string }> {
	/** The open document, or null when there is nothing a reload may replace. */
	capture: () => ReloadCapture | null;
	read: (path: string) => Promise<T>;
	lock: () => Promise<() => void>;
	/** Still the same clean document at the same revision, with no close or preview since. */
	stillValid: (captured: ReloadCapture) => boolean;
	commit: (path: string, content: T) => void;
}

/**
 * Shows the disk's version of the open note when it changed under a clean editor (#191).
 * A conflict choice or a sync run can rewrite the open note; an editor left on the old
 * revision can never save again, and closing the window is a save. Unsaved edits always
 * win: a dirty editor is never replaced.
 */
export async function reloadCleanDocument<T extends { revision: string }>(options: CleanDocumentReloadOptions<T>): Promise<boolean> {
	const captured = options.capture();
	if (!captured) return false;
	let content: T;
	try {
		content = await options.read(captured.path);
	} catch (error) {
		// A note that is gone or unreadable keeps what the editor shows; moves and the trash
		// handle their own notes.
		console.warn('Could not check the open note for changes on disk:', error);
		return false;
	}
	if (content.revision === captured.revision || !options.stillValid(captured)) return false;
	let release: (() => void) | null = null;
	try {
		release = await options.lock();
		if (!options.stillValid(captured)) return false;
		options.commit(captured.path, content);
		return true;
	} finally {
		release?.();
	}
}

/**
 * Takes the editor lock for a close once a reload in flight has settled, since a second lock
 * throws. Null when the close ended while it waited (the 10 s close timeout releases it); a lock
 * taken for a close that already ended is released here, never kept.
 */
export async function lockAfterReload(
	reload: Promise<unknown>,
	lock: () => Promise<(() => void) | null>,
	stillClosing: () => boolean,
): Promise<{ release: (() => void) | null } | null> {
	await Promise.allSettled([reload]);
	if (!stillClosing()) return null;
	const release = await lock();
	if (!stillClosing()) {
		release?.();
		return null;
	}
	return { release };
}
