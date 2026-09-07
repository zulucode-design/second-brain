export function cleanClipUrlInput(input: string): string {
	return input.trim();
}

export function canSubmitWebClip(input: string): boolean {
	return cleanClipUrlInput(input).length > 0;
}

export function webClipFailureMessage(error: unknown): string {
	const message = error instanceof Error ? error.message : String(error);
	return message || 'Could not clip that web page.';
}
