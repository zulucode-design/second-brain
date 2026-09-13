export interface GenerationToken {
	generation: number;
}

/** Small cancellation/generation gate for startup and other delayed replacement work. */
export class GenerationGate {
	private generation = 0;
	private active = true;

	capture(): GenerationToken {
		return { generation: this.generation };
	}

	invalidate(): void {
		this.generation += 1;
	}

	cancel(): void {
		this.active = false;
		this.invalidate();
	}

	isCurrent(token: GenerationToken): boolean {
		return this.active && token.generation === this.generation;
	}
}
