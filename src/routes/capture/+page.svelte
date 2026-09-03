<script lang="ts">
	import { onMount } from 'svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { listen } from '@tauri-apps/api/event';
	import { quickCaptureNote } from '$lib/api';
	import {
		CAPTURE_CATEGORIES,
		captureAction,
		moveSelection,
		type CaptureCategory,
		type CapturePhase
	} from '$lib/utils/quick-capture-policy';

	let text = $state('');
	let phase = $state<CapturePhase>('writing');
	let selected = $state(0);
	let confirmingDiscard = $state(false);
	let error = $state<string | null>(null);
	let saving = $state(false);
	let textarea = $state<HTMLTextAreaElement | null>(null);

	const appWindow = getCurrentWindow();

	/**
	 * Back to an empty overlay before hiding, never after.
	 *
	 * The window is reused for every capture, so anything left behind would be sitting there
	 * the next time the hotkey is pressed — the previous thought, in the way of the new one.
	 */
	async function dismiss() {
		text = '';
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
			await quickCaptureNote(category, text);
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
				phase = 'choosing';
			}
			return;
		}

		const action = captureAction(phase, event, { hasText: text.trim().length > 0, selected });
		if (action.type === 'insert' || action.type === 'none') {
			if (action.type === 'none') event.preventDefault();
			return;
		}
		event.preventDefault();
		switch (action.type) {
			case 'choose':
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
				textarea?.focus();
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
		const shown = listen('quick-capture-shown', () => {
			phase = 'writing';
			confirmingDiscard = false;
			error = null;
			queueMicrotask(() => textarea?.focus());
		});
		queueMicrotask(() => textarea?.focus());
		return () => {
			shown.then((unlisten) => unlisten());
		};
	});
</script>

<svelte:window on:keydown={onKeydown} />

<div class="overlay">
	<textarea
		bind:this={textarea}
		bind:value={text}
		placeholder="Capture a thought…"
		spellcheck="false"
		aria-label="Quick capture"
	></textarea>

	{#if error}
		<p class="error">{error}</p>
	{/if}

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

	textarea {
		flex: 1;
		resize: none;
		border: none;
		outline: none;
		background: transparent;
		color: var(--text-primary);
		font-family: inherit;
		font-size: 1.05rem;
		line-height: 1.5;
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

	.error {
		margin: 0;
		color: var(--text-primary);
		font-size: 0.82rem;
	}
</style>
