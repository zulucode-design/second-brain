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
