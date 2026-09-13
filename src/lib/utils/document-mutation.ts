import type { SaveResult } from './save-coordinator';

export interface ActiveDocumentMutationOptions<T> {
	expectedPath: string;
	currentPath: () => string | null;
	isBlocked: () => boolean;
	prepare: () => Promise<() => void>;
	flush: () => Promise<SaveResult>;
	mutate: () => Promise<T>;
	commit: (value: T) => void;
}

/** Full-operation lock and identity policy shared by direct backend document mutations. */
export async function runActiveDocumentMutation<T>(options: ActiveDocumentMutationOptions<T>): Promise<boolean> {
	if (options.isBlocked() || options.currentPath() !== options.expectedPath) return false;
	let release: (() => void) | null = null;
	try {
		release = await options.prepare();
		if (options.isBlocked() || options.currentPath() !== options.expectedPath) return false;
		const saved = await options.flush();
		if (!saved.ok || options.isBlocked() || options.currentPath() !== options.expectedPath) return false;
		const value = await options.mutate();
		if (options.isBlocked() || options.currentPath() !== options.expectedPath) return false;
		options.commit(value);
		return true;
	} finally {
		release?.();
	}
}
