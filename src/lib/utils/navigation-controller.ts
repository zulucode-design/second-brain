import type { SaveResult } from './save-coordinator';
import type { NoteNavigationResult } from './navigation';

export interface SerializedNavigationOptions<T> {
	isBlocked: () => boolean;
	prepare: () => Promise<() => void>;
	flush: () => Promise<SaveResult>;
	read: (path: string) => Promise<T>;
	commit: (path: string, content: T) => boolean;
	onSaveFailure?: (error: unknown) => void;
	onReadFailure?: (error: unknown) => void;
}

/** Serialized flush/read/final-flush/commit navigation for one editor window. */
export class SerializedNavigationController<T> {
	private queue: Promise<void> = Promise.resolve();
	private readonly options: SerializedNavigationOptions<T>;

	constructor(options: SerializedNavigationOptions<T>) {
		this.options = options;
	}

	navigate(path: string): Promise<NoteNavigationResult> {
		if (!path || this.options.isBlocked()) return Promise.resolve('blocked');
		const run = this.queue.then(() => this.navigateNow(path));
		this.queue = run.then(() => {}, () => {});
		return run;
	}

	private async navigateNow(path: string): Promise<NoteNavigationResult> {
		if (this.options.isBlocked()) return 'blocked';
		let release: (() => void) | null = null;
		try {
			release = await this.options.prepare();
			if (this.options.isBlocked()) return 'blocked';
			const firstSave = await this.options.flush();
			if (!firstSave.ok) {
				this.options.onSaveFailure?.(firstSave.error);
				return 'save-failed';
			}
			if (this.options.isBlocked()) return 'blocked';

			let content: T;
			try {
				content = await this.options.read(path);
			} catch (error) {
				this.options.onReadFailure?.(error);
				return 'not-found';
			}

			if (this.options.isBlocked()) return 'blocked';
			const finalSave = await this.options.flush();
			if (!finalSave.ok) {
				this.options.onSaveFailure?.(finalSave.error);
				return 'save-failed';
			}
			if (this.options.isBlocked()) return 'blocked';
			return this.options.commit(path, content) ? 'navigated' : 'blocked';
		} finally {
			release?.();
		}
	}
}
