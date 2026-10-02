import type { RelocationOutcome } from '$lib/types';
import type { SaveResult } from './save-coordinator';

export async function runSaveGatedAction(
	save: () => Promise<boolean>,
	action: () => Promise<boolean>,
): Promise<boolean> {
	if (!(await save())) return false;
	return action();
}

export interface RelocateDocumentOptions {
	expectedPath: string;
	currentPath: () => string | null;
	prepare: () => Promise<() => void>;
	flush: () => Promise<SaveResult>;
	mutate: () => Promise<RelocationOutcome>;
	rebase: (expectedPath: string, newPath: string, content: NonNullable<RelocationOutcome['note']>) => void;
}

/**
 * Save-before-mutation transaction used by active note and notebook relocations.
 * The backend result is authoritative: adopting it cannot be blocked by a second read
 * after the old path has already vanished.
 */
export async function relocateDocument(options: RelocateDocumentOptions): Promise<string | null> {
	if (options.currentPath() !== options.expectedPath) return null;
	let release: (() => void) | null = null;
	try {
		release = await options.prepare();
		const saved = await options.flush();
		if (!saved.ok || options.currentPath() !== options.expectedPath) return null;
		const outcome = await options.mutate();
		if (!outcome.note) {
			throw new Error('The backend committed a relocation without authoritative note content.');
		}
		options.rebase(options.expectedPath, outcome.path, outcome.note);
		return outcome.path;
	} finally {
		release?.();
	}
}

/**
 * The user-facing message for a save that blocked an action. The current note stays open with
 * its edits; a packaged build has no console, so the dialog is the only report. A note that
 * changed on disk is reported by the editor's own conflict dialog instead (#192).
 */
export function reportSaveFailure(reason: string, result: SaveResult | undefined): boolean {
	if (!result || result.ok) return true;
	console.error(`Save failed before ${reason}:`, result.error);
	if (result.error instanceof DiskConflictError) return false;
	window.alert(`Could not save the current note. ${reason} was cancelled so your edits remain open.\n\n${String(result.error)}`);
	return false;
}

export interface ReportedRelocationOptions extends RelocateDocumentOptions {
	reason: string;
	report: (message: string) => void;
}

/**
 * relocateDocument for a user action, where every way it ends without relocating reaches the
 * user. A failed save is reported by the caller's flush; this reports a note that opened first
 * and a mutation or rebase that failed (#149).
 */
export async function relocateReported(options: ReportedRelocationOptions): Promise<string | null> {
	try {
		const path = await relocateDocument(options);
		if (path === null && options.currentPath() !== options.expectedPath) {
			options.report(`${options.reason} was not applied: another note opened first.`);
		}
		return path;
	} catch (error) {
		console.error(`${options.reason} failed:`, error);
		options.report(`${options.reason} failed: ${String(error)}`);
		return null;
	}
}

/** A preserving save's proof: the draft at `revision` of document `documentVersion` is in `copyPath`. */
export interface PreservationReceipt<T> {
	path: string;
	documentVersion: number;
	revision: number;
	copyPath: string;
	/** The note on disk when the draft was kept, or null when it was moved or deleted. */
	disk: T | null;
}

/**
 * The open note changed or vanished on disk while it had unsaved edits (#192). The edits are
 * safe in a conflict copy, but the note itself was not saved, so the editor stays dirty and
 * every save, navigation and close fails with this until the user chooses a version.
 */
export class DiskConflictError<T = unknown> extends Error {
	readonly receipt: PreservationReceipt<T>;

	constructor(receipt: PreservationReceipt<T>) {
		super('This note changed on disk. Your edits are kept in a conflict copy until you choose a version.');
		this.name = 'DiskConflictError';
		this.receipt = receipt;
	}
}

/** `mine`: my edits become the note, the disk version goes to Trash. `disk`: the reverse. `close`: the note is gone; close it and leave the copy in the conflict list. */
export type DiskConflictChoice = 'mine' | 'disk' | 'close';

export interface CurrentDocument {
	path: string | null;
	documentVersion: number;
	revision: number;
}

export interface ResolveDiskConflictOptions<T> {
	choice: DiskConflictChoice;
	/** An uncommitted title or an open editor menu, which a replacement would lose. */
	drafting: () => boolean;
	lock: () => Promise<() => void>;
	current: () => CurrentDocument;
	flush: () => Promise<SaveResult>;
	/** The sync conflict resolver: `conflict` puts the copy in the note's place, `original` trashes the copy. */
	resolve: (copyPath: string, keep: 'original' | 'conflict') => Promise<void>;
	read: (path: string) => Promise<T>;
	load: (path: string, content: T) => void;
	close: () => void;
}

export type ResolveDiskConflictResult = { ok: true } | { ok: false; message: string };

const DRAFTING = 'Finish or cancel the open edit first.';
const CHANGED = 'The note changed while the choice was being applied. Nothing was replaced; choose again.';

/**
 * Applies the user's choice for a note that changed on disk under unsaved edits. The newest
 * text is proven to be in the conflict copy before anything is replaced: a flush that joined an
 * older in-flight save reports that save's receipt, so it flushes again. Any change to the open
 * document, or a draft appearing, at any await ends it with nothing replaced; the caller keeps
 * the editor frozen meanwhile so that should not happen.
 */
export async function resolveDiskConflict<T>(options: ResolveDiskConflictOptions<T>): Promise<ResolveDiskConflictResult> {
	const fail = (message: string): ResolveDiskConflictResult => ({ ok: false, message });
	if (options.drafting()) return fail(DRAFTING);
	const release = await options.lock();
	try {
		const start = options.current();
		const blocked = () => {
			if (options.drafting()) return DRAFTING;
			const now = options.current();
			const same = now.path === start.path && now.documentVersion === start.documentVersion && now.revision === start.revision;
			return same ? null : CHANGED;
		};
		if (!start.path) return fail(CHANGED);

		let receipt: PreservationReceipt<T> | null = null;
		for (let attempt = 0; attempt < 3 && !receipt; attempt += 1) {
			const before = blocked();
			if (before) return fail(before);
			const result = await options.flush();
			const after = blocked();
			if (after) return fail(after);
			if (result.ok) return fail('The note saved normally, so there is no conflict left to resolve.');
			if (!(result.error instanceof DiskConflictError)) return fail(`Your edits could not be kept: ${String(result.error)}`);
			const preserved = result.error.receipt as PreservationReceipt<T>;
			if (preserved.path !== start.path || preserved.documentVersion !== start.documentVersion) return fail(CHANGED);
			if (preserved.revision === start.revision) receipt = preserved;
		}
		if (!receipt) return fail(CHANGED);

		if (options.choice === 'close') {
			options.close();
			return { ok: true };
		}
		if (options.choice === 'disk' && receipt.disk === null) return fail('The note is no longer on disk. Close it instead.');
		try {
			await options.resolve(receipt.copyPath, options.choice === 'mine' ? 'conflict' : 'original');
		} catch (error) {
			// The copy may have been resolved from Settings meanwhile; a fresh flush keeps the
			// edits in a new copy before the user chooses again.
			await options.flush();
			return fail(`Could not apply the choice: ${String(error)}`);
		}
		const resolved = blocked();
		if (resolved) return fail(resolved);
		let content: T;
		try {
			content = await options.read(start.path);
		} catch (error) {
			return fail(`The choice was applied, but the note could not be read again: ${String(error)}`);
		}
		const loaded = blocked();
		if (loaded) return fail(loaded);
		options.load(start.path, content);
		return { ok: true };
	} finally {
		release();
	}
}
