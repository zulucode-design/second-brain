<script lang="ts">
	import { onMount } from 'svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { listenAppEvent } from '$lib/events';
	import { quickCaptureNote } from '$lib/api';
	import {
		CAPTURE_CATEGORIES,
		MISSING_FIELD_MESSAGE,
		captureAction,
		missingField,
		moveSelection,
		type CaptureCategory,
		type CaptureField,
		type CapturePhase
	} from '$lib/utils/quick-capture-policy';

	let title = $state('');
	let body = $state('');
	let field = $state<CaptureField>('title');
	let phase = $state<CapturePhase>('writing');
	let selected = $state(0);
	let confirmingDiscard = $state(false);
	let error = $state<string | null>(null);
	let saving = $state(false);
	let titleInput = $state<HTMLInputElement | null>(null);
	let bodyInput = $state<HTMLTextAreaElement | null>(null);

	const appWindow = getCurrentWindow();

	/**
	 * Back to an empty overlay before hiding, never after.
	 *
	 * The window is reused for every capture, so anything left behind would be sitting there
	 * the next time the hotkey is pressed — the previous thought, in the way of the new one.
	 */
	function focusField(next: CaptureField) {
		(next === 'title' ? titleInput : bodyInput)?.focus();
	}

	/** Back to the field that is still empty, saying so, instead of a picker that would refuse. */
	function showMissing(missing: CaptureField) {
		error = MISSING_FIELD_MESSAGE[missing];
		phase = 'writing';
		focusField(missing);
	}

	async function dismiss() {
		title = '';
		body = '';
		phase = 'writing';
		selected = 0;
		confirmingDiscard = false;
		error = null;
		await appWindow.hide();
	}

	async function save(category: CaptureCategory) {
		if (saving) return;
		saving = true;
		error = null;
		try {
			await quickCaptureNote(category, title, body);
			await dismiss();
		} catch (cause) {
			// Staying open with the text intact is the point: the thought is still only here.
			error = typeof cause === 'string' ? cause : String(cause);
			phase = 'writing';
		} finally {
			saving = false;
		}
	}

	function onKeydown(event: KeyboardEvent) {
		if (confirmingDiscard) {
			event.preventDefault();
			if (event.key === 'Escape' || event.key.toLowerCase() === 'n') confirmingDiscard = false;
			if (event.key.toLowerCase() === 'd') dismiss();
			// Saving from the discard prompt goes to the picker rather than guessing a
			// category: a note is never filed somewhere the user did not choose.
			if (event.key.toLowerCase() === 's') {
				confirmingDiscard = false;
				const missing = missingField(title, body);
				if (missing) showMissing(missing);
				else phase = 'choosing';
			}
			return;
		}

		const action = captureAction(phase, event, { title, body, field, selected });
		if (action.type === 'insert' || action.type === 'none') {
			if (action.type === 'none') event.preventDefault();
			return;
		}
		event.preventDefault();
		switch (action.type) {
			case 'focus':
				focusField(action.field);
				break;
			case 'missing':
				showMissing(action.field);
				break;
			case 'choose':
				error = null;
				phase = 'choosing';
				break;
			case 'move':
				selected = moveSelection(selected, action.delta);
				break;
			case 'save':
				save(action.category);
				break;
			case 'back':
				phase = 'writing';
				focusField(field);
				break;
			case 'confirmDiscard':
				confirmingDiscard = true;
				break;
			case 'dismiss':
				dismiss();
				break;
		}
	}

	onMount(() => {
		// The window is created hidden at startup so the hotkey never waits on a WebView. It is
		// shown by the backend on activation, and that is when the field needs the caret.
		const shown = listenAppEvent('quickCaptureShown', () => {
			phase = 'writing';
			confirmingDiscard = false;
			error = null;
			queueMicrotask(() => titleInput?.focus());
		});
		queueMicrotask(() => titleInput?.focus());
		return () => {
			shown.then((unlisten) => unlisten());
		};
	});
</script>

<svelte:window on:keydown={onKeydown} />

<div class="overlay">
	<input
		bind:this={titleInput}
		bind:value={title}
		onfocus={() => (field = 'title')}
		placeholder="Title"
		spellcheck="false"
		aria-label="Title"
	/>
	<textarea
		bind:this={bodyInput}
		bind:value={body}
		onfocus={() => (field = 'body')}
		placeholder="Capture a thought…"
		spellcheck="false"
		aria-label="Body"
	></textarea>

	<div class="footer">
		{#if confirmingDiscard}
			<div class="bar">
				<span>Discard this capture?</span>
				<kbd>D</kbd> discard <kbd>S</kbd> save <kbd>Esc</kbd> keep writing
			</div>
		{:else if phase === 'choosing'}
			<div class="bar categories">
				{#each CAPTURE_CATEGORIES as category, index}
					<button
						type="button"
						class:selected={index === selected}
						onclick={() => save(category)}
					>
						<kbd>{index + 1}</kbd>
						{category}
					</button>
				{/each}
			</div>
		{:else}
			<div class="bar hint">
				<kbd>Ctrl</kbd>+<kbd>Enter</kbd> choose a category <kbd>Esc</kbd> dismiss
			</div>
		{/if}

		{#if error}
			<p class="error" role="alert">{error}</p>
		{/if}
	</div>
</div>

<style>
	.overlay {
		display: flex;
		flex-direction: column;
		height: 100vh;
		padding: 14px;
		gap: 10px;
		background: var(--bg-primary);
		border: 1px solid var(--border-color);
		border-radius: 12px;
		box-sizing: border-box;
	}

	input,
	textarea {
		resize: none;
		border: none;
		outline: none;
		background: transparent;
		color: var(--text-primary);
		font-family: inherit;
		font-size: 1.05rem;
		line-height: 1.5;
	}

	input {
		padding-bottom: 8px;
		border-bottom: 1px solid var(--border-color);
		font-weight: 600;
	}

	textarea {
		flex: 1;
	}

	.bar {
		display: flex;
		align-items: center;
		gap: 8px;
		color: var(--text-secondary);
		font-size: 0.82rem;
	}

	.categories button {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 5px 10px;
		border: 1px solid var(--border-color);
		border-radius: 999px;
		background: var(--bg-secondary);
		color: var(--text-secondary);
		font: inherit;
		cursor: pointer;
	}

	.categories button.selected {
		background: var(--bg-active);
		color: var(--text-primary);
		border-color: var(--text-tertiary);
	}

	kbd {
		padding: 1px 5px;
		border: 1px solid var(--border-color);
		border-radius: 4px;
		background: var(--bg-secondary);
		font-family: inherit;
		font-size: 0.76rem;
	}

	.footer {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	/* Bottom right, beside the keys, and larger than them: it is the one thing to read here. */
	.error {
		margin: 0 0 0 auto;
		color: var(--text-primary);
		font-size: 0.95rem;
		font-weight: 600;
		text-align: right;
	}
</style>
