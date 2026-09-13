export interface EditorDocumentIdentity {
	path: string;
	version: number;
}

/**
 * Tracks asynchronous work that may eventually mutate an editor document.
 *
 * A preparation lock rejects new producers, waits for producers that already started,
 * and stays held until the caller releases it. Every commit validates its captured
 * generation. `runAfter` keeps save/version prework outside the tracked set, then admits
 * only the short mutation phase; this avoids a tracked task deadlocking on a flush that
 * drains this barrier while still preventing a late apply after close/navigation.
 */
export class EditorMutationBarrier {
	private path = '';
	private version = 0;
	private locked = false;
	private readonly tasks = new Set<Promise<void>>();

	setDocument(path: string): void {
		this.path = path;
		this.version += 1;
	}

	capture(): EditorDocumentIdentity {
		return { path: this.path, version: this.version };
	}

	isCurrent(identity: EditorDocumentIdentity): boolean {
		return identity.path === this.path && identity.version === this.version;
	}

	private track(identity: EditorDocumentIdentity, producer: (identity: EditorDocumentIdentity) => Promise<void>): Promise<void> {
		let task: Promise<void>;
		task = Promise.resolve()
			.then(() => producer(identity))
			.finally(() => this.tasks.delete(task));
		this.tasks.add(task);
		return task;
	}

	start(producer: (identity: EditorDocumentIdentity) => Promise<void>): boolean {
		if (this.locked || !this.path) return false;
		void this.track(this.capture(), producer);
		return true;
	}

	/** Admit and await one fully tracked producer. */
	async run(producer: (identity: EditorDocumentIdentity) => Promise<void>): Promise<boolean> {
		if (this.locked || !this.path) return false;
		await this.track(this.capture(), producer);
		return true;
	}

	/**
	 * Run potentially flush-dependent prework without joining the drain set, then admit
	 * a generation-checked mutation. If close/navigation locked or replaced the document
	 * while prework was pending, the mutation is cancelled.
	 */
	async runAfter<T>(
		prework: () => Promise<T>,
		commit: (value: T, identity: EditorDocumentIdentity) => Promise<void> | void,
	): Promise<boolean> {
		if (this.locked || !this.path) return false;
		const identity = this.capture();
		const value = await prework();
		if (this.locked || !this.isCurrent(identity)) return false;
		await this.track(identity, async (trackedIdentity) => {
			if (!this.isCurrent(trackedIdentity)) return;
			await commit(value, trackedIdentity);
		});
		return true;
	}

	async drain(): Promise<void> {
		while (this.tasks.size > 0) {
			await Promise.allSettled([...this.tasks]);
		}
	}

	async lockAndDrain(): Promise<() => void> {
		if (this.locked) throw new Error('Editor mutation preparation is already in progress.');
		this.locked = true;
		await this.drain();
		let released = false;
		return () => {
			if (released) return;
			released = true;
			this.locked = false;
		};
	}

	isLocked(): boolean {
		return this.locked;
	}
}
