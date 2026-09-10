/**
 * Wording for the Notion panel, kept apart from the markup so it can be tested.
 *
 * The panel's whole job is answering "is it working, and if not, why not". A number with no
 * reason is not an answer, so every count that means "a note is not in Notion" carries the
 * reason with it.
 */

/** What a publish run did. Mirrors `publish::Summary` in the backend. */
export interface NotionSummary {
	created: number;
	updated: number;
	moved: number;
	trashed: number;
	up_to_date: number;
	failed: number;
	skipped: Record<string, number>;
}

/** Mirrors `notion::commands::NotionStatus`. Never carries the token. */
export interface NotionStatus {
	enabled: boolean;
	connected: boolean;
	connection_name: string | null;
	setup_complete: boolean;
	poll_minutes: number;
	last_run: string | null;
	last_summary: NotionSummary | null;
	last_error: string | null;
	failing_notes: number;
}

export interface VisiblePage {
	id: string;
	title: string;
}

/** Why a note was not published, phrased for someone who has not read the design. */
const SKIP_REASONS: Record<string, string> = {
	uncategorised: 'uncategorised',
	conflict_copy: 'sync conflict copies',
	no_database: 'setup unfinished for their category'
};

/** One sentence describing what a run did. */
export function describeSummary(summary: NotionSummary): string {
	const parts: string[] = [];
	if (summary.created) parts.push(`${summary.created} published`);
	if (summary.updated) parts.push(`${summary.updated} updated`);
	if (summary.moved) parts.push(`${summary.moved} moved`);
	if (summary.trashed) parts.push(`${summary.trashed} removed`);
	if (summary.failed) parts.push(`${summary.failed} failed`);

	// A run that changed nothing is the normal case every five minutes, and should read as
	// healthy rather than as nothing having happened.
	return parts.length ? `${parts.join(', ')}.` : 'Everything is up to date.';
}

/**
 * The notes deliberately left out, with why.
 *
 * Returns nothing when nothing was skipped, so the panel shows no line rather than a
 * reassuring "0 skipped" that someone has to read past.
 */
export function describeSkipped(summary: NotionSummary): string | null {
	const entries = Object.entries(summary.skipped ?? {}).filter(([, count]) => count > 0);
	if (!entries.length) return null;

	const parts = entries.map(([reason, count]) => `${count} ${SKIP_REASONS[reason] ?? reason}`);
	const total = entries.reduce((sum, [, count]) => sum + count, 0);
	const noun = total === 1 ? 'note' : 'notes';
	return `${total} ${noun} not published: ${parts.join(', ')}.`;
}

/**
 * What to tell the user to do next, or nothing when there is nothing to do.
 *
 * Ordered by what blocks what: without a token nothing else matters, and without the
 * databases no note has anywhere to go.
 */
export function nextStep(status: NotionStatus): string | null {
	if (!status.connected) return 'Connect an integration token to begin.';
	if (!status.setup_complete) return 'Choose the Notion page your four databases will live under.';
	if (status.last_error) return status.last_error;
	return null;
}
