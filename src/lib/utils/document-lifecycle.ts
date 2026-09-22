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
 * its edits; a packaged build has no console, so the dialog is the only report.
 */
export function reportSaveFailure(reason: string, result: SaveResult | undefined): boolean {
	if (!result || result.ok) return true;
	console.error(`Save failed before ${reason}:`, result.error);
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
