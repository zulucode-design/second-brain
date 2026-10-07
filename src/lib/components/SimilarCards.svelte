<script lang="ts">
	import { appendSimilar, dismissSimilar, similarCardFailed, similarCards } from '$lib/stores/similar';
	import type { SimilarMatch } from '$lib/types';
	import { similarCoverage, type AppendOutcome, type SimilarCard } from '$lib/utils/similar';

	let {
		onOpen,
		onAppend,
	}: {
		/** Opens the note; false when it could not be opened. */
		onOpen: (path: string) => Promise<boolean>;
		/** Runs `append` with either note, if open, saved and closed first and reopened after
		 * (unless it went to trash). */
		onAppend: (capturePath: string, targetPath: string, append: () => Promise<AppendOutcome>) => Promise<void>;
	} = $props();

	function append(card: SimilarCard, match: SimilarMatch) {
		const capture = card.check.capture;
		return onAppend(capture.path, match.path, () => appendSimilar(card.id, capture, match));
	}

	// Opening the note to edit it is one of the card's three answers, so the card goes once the
	// note is open.
	async function open(card: SimilarCard, match: SimilarMatch) {
		if (await onOpen(match.path)) dismissSimilar(card.id);
		else similarCardFailed(card.id, 'That note could not be opened.');
	}
</script>

{#if $similarCards.length > 0}
	<div class="similar-cards" role="region" aria-label="Similar notes">
		{#each $similarCards as card (card.id)}
			{@const coverage = similarCoverage(card.check)}
			<section class="similar-card">
				<div class="similar-heading">
					<span>“{card.check.capture.title}” is similar to</span>
				</div>
				{#each card.check.matches as match (match.path)}
					<div class="similar-match">
						<div class="similar-title">
							<span>{match.title}</span>
							<span class="similar-label">Similar</span>
						</div>
						<p class="similar-excerpt" title={match.excerpt}>{match.excerpt}</p>
						<div class="similar-actions">
							<button disabled={card.busy} onclick={() => append(card, match)}>Append</button>
							<button disabled={card.busy} onclick={() => open(card, match)}>Open</button>
						</div>
					</div>
				{/each}
				{#if coverage}<div class="similar-coverage">{coverage}</div>{/if}
				{#if card.error}<div class="similar-error" role="alert">{card.error}</div>{/if}
				<button class="similar-dismiss" disabled={card.busy} onclick={() => dismissSimilar(card.id)}>
					Dismiss
				</button>
			</section>
		{/each}
	</div>
{/if}

<style>
	.similar-cards {
		position: fixed;
		right: 16px;
		bottom: 16px;
		width: 340px;
		max-height: calc(100vh - 96px);
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: 10px;
		/* Over the search, palette, info, and settings panels (2000), so a notification click that
		   brings the window forward shows the cards; under the editor's own dialogs (2100 and up). */
		z-index: 2050;
	}

	.similar-card {
		background: var(--bg-secondary);
		border: 1px solid var(--border-color);
		border-radius: 10px;
		box-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
		padding: 10px 12px;
		font-size: 13px;
		color: var(--text-primary);
	}

	.similar-heading {
		color: var(--text-secondary);
		font-size: 12px;
		margin-bottom: 6px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.similar-match {
		border-top: 1px solid var(--border-light);
		padding: 8px 0;
	}

	.similar-title {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		font-weight: 600;
	}

	.similar-label {
		flex-shrink: 0;
		font-size: 11px;
		font-weight: 500;
		color: var(--text-accent);
		background: color-mix(in srgb, var(--accent) 14%, transparent);
		border-radius: 4px;
		padding: 0 5px;
	}

	.similar-excerpt {
		margin: 4px 0 6px;
		color: var(--text-secondary);
		line-height: 1.45;
		display: -webkit-box;
		-webkit-line-clamp: 5;
		line-clamp: 5;
		-webkit-box-orient: vertical;
		overflow: hidden;
		user-select: text;
	}

	.similar-actions {
		display: flex;
		gap: 6px;
	}

	.similar-actions button,
	.similar-dismiss {
		border: 1px solid var(--border-color);
		background: var(--bg-primary);
		color: var(--text-secondary);
		border-radius: 6px;
		padding: 3px 9px;
		font: inherit;
		font-size: 12px;
		cursor: pointer;
	}

	.similar-actions button:hover,
	.similar-dismiss:hover {
		background: var(--bg-hover);
	}

	.similar-coverage {
		font-size: 11px;
		color: var(--text-tertiary);
		padding: 4px 0;
	}

	.similar-error {
		color: var(--danger);
		font-size: 12px;
		padding: 4px 0;
	}

	.similar-dismiss {
		margin-top: 4px;
	}
</style>
