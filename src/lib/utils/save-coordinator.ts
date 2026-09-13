export interface SaveSnapshotBase {
	path: string;
}

export type VersionedSaveSnapshot<T extends SaveSnapshotBase> = T & {
	revision: number;
	documentVersion: number;
};

export type SaveResult =
	| { ok: true; status: 'clean' | 'saved'; revision: number }
	| { ok: false; status: 'failed'; revision: number; error: unknown };

export interface SaveCoordinatorOptions<T extends SaveSnapshotBase> {
	delayMs: number;
	prepare?: () => Promise<void>;
	capture: () => T | null;
	persist: (snapshot: VersionedSaveSnapshot<T>) => Promise<void>;
	onDirtyChange: (dirty: boolean) => void;
	onAutoSaveError?: (error: unknown) => void;
	setTimer?: (callback: () => void, delayMs: number) => ReturnType<typeof setTimeout>;
	clearTimer?: (timer: ReturnType<typeof setTimeout>) => void;
}

/**
 * Owns the complete serialization tail for one editor instance.
 *
 * A revision belongs to one document version. Successful persistence only clears dirty
 * when both still match, and a drain loops when edits arrive while an older revision is
 * in flight. All callers (debounce, manual save, navigation, and close) therefore await
 * the same queue rather than creating competing save paths.
 */
export class SaveCoordinator<T extends SaveSnapshotBase> {
	private readonly options: SaveCoordinatorOptions<T>;
	private readonly setTimer: NonNullable<SaveCoordinatorOptions<T>['setTimer']>;
	private readonly clearTimer: NonNullable<SaveCoordinatorOptions<T>['clearTimer']>;
	private timer: ReturnType<typeof setTimeout> | null = null;
	private queue: Promise<void> = Promise.resolve();
	private drainPromise: Promise<SaveResult> | null = null;
	private documentPath: string | null = null;
	private documentVersion = 0;
	private revision = 0;
	private persistedRevision = 0;

	constructor(options: SaveCoordinatorOptions<T>) {
		this.options = options;
		this.setTimer = options.setTimer ?? ((callback, delayMs) => setTimeout(callback, delayMs));
		this.clearTimer = options.clearTimer ?? ((timer) => clearTimeout(timer));
	}

	setDocument(path: string | null, forceReset = false): void {
		if (!forceReset && path === this.documentPath) return;
		this.cancelDebounce();
		this.documentPath = path;
		this.documentVersion += 1;
		this.revision = 0;
		this.persistedRevision = 0;
		this.options.onDirtyChange(false);
	}

	rebaseDocument(expectedPath: string, newPath: string): void {
		if (this.documentPath !== expectedPath) {
			throw new Error(`Save invariant failed: expected active document ${expectedPath}, coordinator owns ${this.documentPath ?? 'none'}.`);
		}
		if (this.isDirty() || this.drainPromise) {
			throw new Error('Save invariant failed: cannot rebase a document with pending revisions.');
		}
		this.setDocument(newPath, true);
	}

	markDirty(): number {
		if (!this.documentPath) return this.revision;
		this.revision += 1;
		this.options.onDirtyChange(true);
		this.schedule();
		return this.revision;
	}

	isDirty(): boolean {
		return this.revision > this.persistedRevision;
	}

	getRevision(): number {
		return this.revision;
	}

	cancelDebounce(): void {
		if (this.timer === null) return;
		this.clearTimer(this.timer);
		this.timer = null;
	}

	async flush(): Promise<SaveResult> {
		this.cancelDebounce();
		if (!this.isDirty()) {
			return { ok: true, status: 'clean', revision: this.revision };
		}
		if (!this.drainPromise) {
			this.drainPromise = this.drain().finally(() => {
				this.drainPromise = null;
			});
		}
		return this.drainPromise;
	}

	private schedule(): void {
		this.cancelDebounce();
		this.timer = this.setTimer(() => {
			this.timer = null;
			void this.flush().then((result) => {
				if (!result.ok) this.options.onAutoSaveError?.(result.error);
			});
		}, this.options.delayMs);
	}

	private async drain(): Promise<SaveResult> {
		while (this.isDirty()) {
			const requestedDocumentVersion = this.documentVersion;
			const requestedRevision = this.revision;
			try {
				await this.options.prepare?.();
				if (requestedDocumentVersion !== this.documentVersion) {
					if (this.isDirty()) continue;
					return { ok: true, status: 'clean', revision: this.revision };
				}
				// Preparation may yield while another edit (notably a pasted blob) arrives.
				// Prepare that newer revision in a fresh pass instead of capturing content that
				// the completed pass never made persistence-safe.
				if (requestedRevision !== this.revision) continue;

				const revision = requestedRevision;
				const captured = this.options.capture();
				if (!captured) {
					throw new Error('The current note cannot be saved.');
				}
				if (captured.path !== this.documentPath) {
					throw new Error(`Save invariant failed: captured ${captured.path}, coordinator owns ${this.documentPath ?? 'none'}.`);
				}
				const snapshot = {
					...captured,
					revision,
					documentVersion: requestedDocumentVersion,
				} as VersionedSaveSnapshot<T>;

				const queued = this.queue.then(() => this.options.persist(snapshot));
				this.queue = queued.catch(() => {});
				await queued;

				if (
					this.documentVersion === snapshot.documentVersion
					&& this.documentPath === snapshot.path
				) {
					this.persistedRevision = Math.max(this.persistedRevision, snapshot.revision);
					if (this.revision === snapshot.revision) {
						this.options.onDirtyChange(false);
						return { ok: true, status: 'saved', revision: snapshot.revision };
					}
					// A newer edit arrived during persistence. Keep dirty and capture it next.
					this.options.onDirtyChange(true);
					continue;
				}

				if (this.documentVersion === snapshot.documentVersion) {
					throw new Error(`Save invariant failed: persisted ${snapshot.path}, coordinator owns ${this.documentPath ?? 'none'}.`);
				}

				// A deliberate document replacement happened while persistence was in flight.
				// Its own revisions, if any, are drained next without clearing them here.
				if (this.isDirty()) continue;
				return { ok: true, status: 'saved', revision: snapshot.revision };
			} catch (error) {
				this.options.onDirtyChange(this.isDirty());
				return { ok: false, status: 'failed', revision: this.revision, error };
			}
		}

		return { ok: true, status: 'clean', revision: this.revision };
	}
}
