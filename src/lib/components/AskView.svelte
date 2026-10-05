<script lang="ts">
	import MarkdownIt from 'markdown-it';
	import { readNote } from '$lib/api';
	import { appConfig } from '$lib/stores/app';
	import { askAnswers, stop } from '$lib/stores/ask';
	import {
		coverageLabel,
		createAnswerRenderer,
		askUnavailableReason,
		expandedAnswer,
		isRunning,
		nextSourceIndex,
		openCitation,
		stageLabel,
		visibleUnread,
		withMissing,
		UNREAD_SHOWN,
		type AskAnswer,
	} from '$lib/utils/ask';
	import type { NoteNavigationResult } from '$lib/utils/navigation';

	let { onOpen }: { onOpen: (path: string, noteId: string | null) => Promise<NoteNavigationResult> } = $props();

	const render = createAnswerRenderer(MarkdownIt);
	const answers = $derived($askAnswers);
	const unavailable = $derived(askUnavailableReason($appConfig?.ai_provider));

	let picked = $state<{ id: string; newest: string } | null>(null);
	let missing = $state<Set<string>>(new Set());
	let showAllUnread = $state<Set<string>>(new Set());
	let now = $state(Date.now());
	let listEl = $state<HTMLDivElement>(null!);

	const openId = $derived(expandedAnswer(answers, picked));

	$effect(() => {
		if (!answers.some(isRunning)) return;
		const timer = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(timer);
	});

	async function open(path: string, noteId: string | null) {
		missing = withMissing(missing, path, await openCitation(path, noteId, readNote, onOpen));
	}

	function clickCitation(event: MouseEvent, answer: AskAnswer) {
		const target = (event.target as HTMLElement).closest<HTMLButtonElement>('button.citation');
		const source = answer.plan?.sources[Number(target?.dataset.source) - 1];
		if (source) void open(source.path, source.note_id);
	}

	/** Moves focus into the open answer's sources; false when it has none. */
	export function focusSources(): boolean {
		const first = listEl?.querySelector<HTMLButtonElement>('.ask-answer.open .source');
		first?.focus();
		return Boolean(first);
	}

	function handleKeydown(event: KeyboardEvent) {
		const sources = [...(listEl?.querySelectorAll<HTMLButtonElement>('.ask-answer.open .source') ?? [])];
		const next = nextSourceIndex(sources.indexOf(document.activeElement as HTMLButtonElement), sources.length, event.key);
		if (next === null) return;
		event.preventDefault();
		sources[next].focus();
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="ask" bind:this={listEl} onkeydown={handleKeydown}>
	{#if unavailable}
		<div class="ask-empty">{unavailable}. Choose one in Settings to use Ask.</div>
	{:else if answers.length === 0}
		<div class="ask-empty">
			Ask a question about your notes and press <kbd>&crarr;</kbd>. Answers cite the notes they come from.
		</div>
	{/if}

	{#each answers as answer (answer.id)}
		{@const isOpen = answer.id === openId}
		<section class="ask-answer" class:open={isOpen}>
			<button class="ask-question" onclick={() => (picked = { id: answer.id, newest: answers[0].id })} aria-expanded={isOpen}>
				{answer.question}
			</button>
			{#if isOpen}
				{#if isRunning(answer)}
					<div class="ask-progress">
						<span>{stageLabel(answer, now)}</span>
						<button class="ask-stop" onclick={() => stop(answer.id)}>Stop</button>
					</div>
				{/if}
				{#if answer.plan && answer.plan.queuedNotes > 0}
					<div class="ask-warning">
						{answer.plan.queuedNotes} {answer.plan.queuedNotes === 1 ? 'note is' : 'notes are'} still being indexed, so this answer may miss them.
					</div>
				{/if}
				{#if answer.status === 'no-match'}
					<div class="ask-empty">No notes about this.</div>
				{/if}
				{#if answer.text}
					<!-- svelte-ignore a11y_click_events_have_key_events -->
					<div class="ask-text" onclick={(event) => clickCitation(event, answer)}>
						{@html render(answer.text, answer.plan?.sources.length ?? 0)}
					</div>
				{/if}
				{#if answer.status === 'stopped'}
					<div class="ask-note">Stopped.</div>
				{/if}
				{#if answer.error}
					<div class="ask-error">{answer.error}</div>
				{/if}
				{#if answer.plan && answer.plan.sources.length > 0}
					<div class="ask-sources">
						<div class="ask-coverage">{coverageLabel(answer.plan, answer.status === 'error')}</div>
						{#each answer.plan.sources as source (source.number)}
							<button class="source" onclick={() => open(source.path, source.note_id)}>
								<span class="source-number">{source.number}</span>
								<span class="source-title">{source.title}</span>
								{#if missing.has(source.path)}<span class="source-missing">note no longer exists</span>{/if}
							</button>
						{/each}
						{#if answer.plan.unread.length > 0}
							<div class="ask-coverage">Related but not read</div>
							{#each visibleUnread(answer.plan.unread, showAllUnread.has(answer.id)) as note (note.path)}
								<button class="source unread" onclick={() => open(note.path, note.note_id)}>
									<span class="source-title">{note.title}</span>
									{#if missing.has(note.path)}<span class="source-missing">note no longer exists</span>{/if}
								</button>
							{/each}
							{#if answer.plan.unread.length > UNREAD_SHOWN && !showAllUnread.has(answer.id)}
								<button class="ask-more" onclick={() => (showAllUnread = new Set(showAllUnread).add(answer.id))}>
									Show all {answer.plan.unread.length}
								</button>
							{/if}
						{/if}
					</div>
				{/if}
			{/if}
		</section>
	{/each}
</div>

<style>
	.ask {
		overflow-y: auto;
		padding: 6px 12px 12px;
		flex: 1;
	}

	.ask-empty,
	.ask-note {
		padding: 16px 8px;
		color: var(--text-tertiary);
		font-size: 13px;
		text-align: center;
	}

	.ask-empty kbd {
		font-size: 11px;
		background: var(--bg-tertiary);
		border: 1px solid var(--border-color);
		border-radius: 4px;
		padding: 0 5px;
		font-family: inherit;
	}

	.ask-answer {
		border-bottom: 1px solid var(--border-light);
		padding: 6px 0;
	}

	.ask-answer:last-child {
		border-bottom: none;
	}

	.ask-question {
		display: block;
		width: 100%;
		text-align: left;
		border: none;
		background: none;
		padding: 6px 8px;
		border-radius: 6px;
		font: inherit;
		font-size: 13px;
		color: var(--text-secondary);
		cursor: pointer;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.ask-answer.open .ask-question {
		font-weight: 600;
		color: var(--text-primary);
		white-space: normal;
	}

	.ask-question:hover {
		background: var(--bg-hover);
	}

	.ask-progress {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 4px 8px;
		font-size: 12px;
		color: var(--text-tertiary);
	}

	.ask-stop {
		border: 1px solid var(--border-color);
		background: var(--bg-primary);
		color: var(--text-secondary);
		border-radius: 6px;
		padding: 3px 9px;
		font: inherit;
		font-size: 12px;
		cursor: pointer;
	}

	.ask-warning {
		margin: 4px 8px;
		font-size: 12px;
		color: var(--text-secondary);
		background: var(--bg-tertiary);
		border-radius: 6px;
		padding: 6px 9px;
	}

	.ask-error {
		padding: 6px 8px;
		color: var(--error, #c94b5d);
		font-size: 13px;
		line-height: 1.5;
	}

	.ask-text {
		padding: 4px 8px;
		font-size: 14px;
		line-height: 1.55;
		color: var(--text-primary);
		user-select: text;
	}

	.ask-text :global(p) {
		margin: 0 0 8px;
	}

	.ask-text :global(ul),
	.ask-text :global(ol) {
		margin: 0 0 8px;
		padding-left: 20px;
	}

	.ask-text :global(.citation) {
		border: none;
		background: color-mix(in srgb, var(--accent) 14%, transparent);
		color: var(--text-accent);
		border-radius: 4px;
		padding: 0 3px;
		margin: 0 1px;
		font: inherit;
		font-size: 12px;
		cursor: pointer;
	}

	.ask-sources {
		padding: 6px 4px 2px;
	}

	.ask-coverage {
		font-size: 11px;
		color: var(--text-tertiary);
		padding: 6px 4px 4px;
	}

	.ask-more {
		border: none;
		background: none;
		padding: 5px 6px;
		font: inherit;
		font-size: 12px;
		color: var(--text-accent);
		cursor: pointer;
	}

	.source {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		text-align: left;
		border: none;
		background: none;
		padding: 5px 6px;
		border-radius: 6px;
		font: inherit;
		font-size: 13px;
		color: var(--text-primary);
		cursor: pointer;
	}

	.source:hover,
	.source:focus-visible {
		background: var(--bg-hover);
		outline: none;
	}

	.source.unread {
		color: var(--text-secondary);
	}

	.source-number {
		font-size: 11px;
		color: var(--text-accent);
		background: color-mix(in srgb, var(--accent) 14%, transparent);
		border-radius: 4px;
		padding: 0 5px;
		flex-shrink: 0;
	}

	.source-title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.source-missing {
		margin-left: auto;
		font-size: 11px;
		color: var(--error, #c94b5d);
		flex-shrink: 0;
	}
</style>
