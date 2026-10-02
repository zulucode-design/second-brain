<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { listenAppEvent } from '$lib/events';
	import { getCurrentWebview } from '@tauri-apps/api/webview';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import Editor from './Editor.svelte';
	import {
		appConfig,
		activeNote,
		activeNotePath,
		editorDirty,
		readOnly,
		shutdownPending,
		sourceMode
	} from '$lib/stores/app';
	import { readNote } from '$lib/api';
	import { NAVIGATE_NOTE_EVENT, type NavigateNoteRequest, type NoteNavigationResult } from '$lib/utils/navigation';
	import { SerializedNavigationController } from '$lib/utils/navigation-controller';
	import { GenerationGate } from '$lib/utils/generation-gate';
	import { DiskConflictError } from '$lib/utils/document-lifecycle';
	import { lockAfterReload, reloadCleanDocument, RELOAD_RETRY_MS } from '$lib/utils/document-reload';
	import { debounce } from '$lib/utils/debounce';
	import { keybindings, matchAction } from '$lib/keybindings';
	import type { RelocationOutcome } from '$lib/types';

	let { notePath }: { notePath: string } = $props();

	const appWindow = getCurrentWindow();
	const appWebview = getCurrentWebview();
	let editor = $state<Editor>(null!);
	let unlistenFileChange: (() => void) | null = null;
	let unlistenSyncDone: (() => void) | null = null;
	// The note reload running now; a close waits it out before taking the editor lock.
	let openNoteReload: Promise<unknown> = Promise.resolve();
	let openNoteReloadRetry: ReturnType<typeof setTimeout> | null = null;
	let unlistenUiScale: (() => void) | null = null;
	let removeNavigationRequest: (() => void) | null = null;
	let closingRequestId: string | null = null;
	let releaseCloseMutationLock: (() => void) | null = null;
	let readOnlyBeforeClose = false;
	let currentPath = $state('');
	let maximized = $state(false);
	let loadError = $state<string | null>(null);
	const lifetimeGate = new GenerationGate();
	const initialLoadGate = new GenerationGate();

	async function checkMaximized() {
		maximized = await appWindow.isMaximized();
	}

	$effect(() => {
		checkMaximized();
		const unlisten = appWindow.onResized(() => checkMaximized());
		return () => { unlisten.then(fn => fn()); };
	});

	let lastMouseDown = 0;
	const RESIZE_EDGE = 6;

	function handleMouseDown(e: MouseEvent) {
		if (e.button !== 0) return;
		const target = e.target as HTMLElement;
		if (target.closest('.titlebar-controls') || target.closest('.titlebar-actions')) return;

		if (!maximized) {
			const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
			if (e.clientY - rect.top < RESIZE_EDGE || e.clientX - rect.left < RESIZE_EDGE) return;
		}

		const now = Date.now();
		if (now - lastMouseDown < 300) {
			appWindow.toggleMaximize();
			lastMouseDown = 0;
			return;
		}
		lastMouseDown = now;
		appWindow.startDragging();
	}

	const navigationController = new SerializedNavigationController<Awaited<ReturnType<typeof readNote>>>({
		isBlocked: () => $shutdownPending,
		prepare: async () => editor ? editor.lockMutations() : () => {},
		flush: async () => editor?.flushSave() ?? { ok: true, status: 'clean', revision: 0 },
		read: readNote,
		commit: (path, content) => {
			if ($shutdownPending) return false;
			currentPath = path;
			$activeNote = content;
			$activeNotePath = path;
			$editorDirty = false;
			editor?.loadNote(path, content.content, undefined, false, content.revision);
			void appWindow.setTitle(`${content.meta.title} - Second Brain`);
			return true;
		},
		onSaveFailure: (error) => {
			console.error('Save failed before note-window navigation:', error);
			// The editor's conflict dialog explains a note that changed on disk (#192).
			if (!(error instanceof DiskConflictError)) window.alert(`Could not save this note. Navigation was cancelled so your edits remain open.\n\n${String(error)}`);
		},
		onReadFailure: (error) => console.error('Failed to navigate note window:', error),
	});

	// Same contract as the main window (#191): a clean editor shows the disk's version of its
	// note after a conflict choice or a sync run rewrites it. Unsaved edits are never replaced; a
	// draft or an open editor dialog delays it.
	function reloadOpenNoteFromDisk(alive: () => boolean): Promise<void> {
		const run = navigationController.enqueue(async () => {
			const current = editor;
			if (!current || !alive()) return 'skipped' as const;
			const reloadable = () => alive() && editor === current && !$shutdownPending && !$editorDirty;
			const reload = reloadCleanDocument({
				capture: () => reloadable() && $activeNotePath ? { path: $activeNotePath, revision: current.getLoadedRevision() } : null,
				drafting: () => current.hasPendingDraft(),
				read: readNote,
				lock: () => current.lockMutations(),
				stillValid: (captured) => reloadable() && $activeNotePath === captured.path && current.getLoadedRevision() === captured.revision,
				commit: (path, content) => {
					$activeNote = content;
					current.loadNote(path, content.content, undefined, false, content.revision, true);
					void appWindow.setTitle(`${content.meta.title} - Second Brain`);
				},
			});
			openNoteReload = reload;
			return reload;
		});
		return run.then((outcome) => {
			if (outcome !== 'deferred' || openNoteReloadRetry || !alive()) return;
			openNoteReloadRetry = setTimeout(() => {
				openNoteReloadRetry = null;
				if (alive()) void reloadOpenNoteFromDisk(alive);
			}, RELOAD_RETRY_MS);
		}, (error) => console.error('Failed to reload the note window:', error));
	}

	// A rename takes the editor lock too, so it waits its turn with navigation and reloads.
	function relocateInTurn(path: string, reason: string, mutation: () => Promise<RelocationOutcome>): Promise<string | null> {
		return navigationController.enqueue(async () => editor ? editor.relocateUnqueued(path, reason, mutation) : null);
	}

	async function navigateToPathResult(path: string): Promise<NoteNavigationResult> {
		initialLoadGate.invalidate();
		if (path === currentPath) return 'navigated';
		return navigationController.navigate(path);
	}

	async function navigateToPath(path: string): Promise<boolean> {
		return (await navigateToPathResult(path)) === 'navigated';
	}

	function handleKeydown(e: KeyboardEvent) {
		if ($shutdownPending) {
			e.preventDefault();
			return;
		}
		const action = matchAction(e, $keybindings);
		if (action === 'save') {
			e.preventDefault();
			editor?.forceSave();
		} else if (action === 'toggle-source') {
			e.preventDefault();
			$sourceMode = !$sourceMode;
		}
	}

	async function applyUiScale(scale: number) {
		try {
			await appWebview.setZoom(scale);
		} catch (e) {
			console.error('Failed to apply interface scale to note window:', e);
		}
	}

	export async function prepareForClose(requestId: string): Promise<boolean> {
		if (closingRequestId && closingRequestId !== requestId) return false;
		if (!closingRequestId) {
			closingRequestId = requestId;
			readOnlyBeforeClose = $readOnly;
			$shutdownPending = true;
			$readOnly = true;
			await tick();
			try {
				// A reload sees the close and ends without loading; its lock goes first.
				const held = await lockAfterReload(
					openNoteReload,
					async () => editor ? editor.lockMutations() : null,
					() => closingRequestId === requestId && $shutdownPending,
				);
				if (!held) return false;
				releaseCloseMutationLock = held.release;
			} catch (error) {
				console.error('Could not prepare note window for close:', error);
				return false;
			}
		}
		const result = await editor?.flushSave();
		const saved = !result || result.ok;
		if (!saved) {
			console.error('Save failed before closing note window:', result.error);
			// The editor's conflict dialog explains a note that changed on disk (#192).
			if (!(result.error instanceof DiskConflictError)) window.alert(`Could not save this note. The window will remain open so your edits are not lost.\n\n${String(result.error)}`);
		}
		return saved;
	}

	export function releaseClose(requestId: string) {
		if (closingRequestId !== requestId) return;
		releaseCloseMutationLock?.();
		releaseCloseMutationLock = null;
		$readOnly = readOnlyBeforeClose;
		$shutdownPending = false;
		closingRequestId = null;
	}

	onMount(async () => {
		const lifetime = lifetimeGate.capture();
		const initialLoad = initialLoadGate.capture();
		const alive = () => lifetimeGate.isCurrent(lifetime);
		const initialIsCurrent = () => alive() && initialLoadGate.isCurrent(initialLoad);
		const handleNavigationRequest = (event: Event) => {
			const path = (event as CustomEvent<NavigateNoteRequest>).detail?.path;
			if (path) void navigateToPath(path);
		};
		window.addEventListener(NAVIGATE_NOTE_EVENT, handleNavigationRequest);
		removeNavigationRequest = () => window.removeEventListener(NAVIGATE_NOTE_EVENT, handleNavigationRequest);

		const uiScaleUnlisten = await listenAppEvent('uiScaleChanged', (event) => {
			if (alive()) void applyUiScale(event.payload);
		});
		if (!alive()) { uiScaleUnlisten(); return; }
		unlistenUiScale = uiScaleUnlisten;
		await applyUiScale($appConfig?.ui_scale ?? 1);
		if (!alive()) return;

		try {
			const content = await readNote(notePath);
			if (!initialIsCurrent()) return;
			currentPath = notePath;
			$activeNote = content;
			$activeNotePath = notePath;
			$editorDirty = false;
			await tick();
			if (!initialIsCurrent() || currentPath !== notePath) return;
			editor?.loadNote(notePath, content.content, undefined, false, content.revision);
			void appWindow.setTitle(`${content.meta.title} - Second Brain`);
		} catch (e) {
			if (initialIsCurrent()) loadError = String(e);
		}
		if (!alive()) return;

		// Any event type and path: a conflict choice replaces the note by renaming onto it, and
		// watcher paths need not match byte for byte. The revision check ignores our own saves.
		const debouncedReload = debounce(() => { if (alive()) void reloadOpenNoteFromDisk(alive); }, 300);
		const fileChangeUnlisten = await listenAppEvent('fileChanged', () => {
			if (alive()) debouncedReload();
		});
		if (!alive()) { fileChangeUnlisten(); return; }
		unlistenFileChange = fileChangeUnlisten;
		// A sync run suppresses the watcher, so its end is the only signal for what it rewrote.
		const syncDoneUnlisten = await listenAppEvent('syncDone', (event) => {
			if (alive() && event.payload.outcome !== 'failure') void reloadOpenNoteFromDisk(alive);
		});
		if (!alive()) { syncDoneUnlisten(); return; }
		unlistenSyncDone = syncDoneUnlisten;
	});

	onDestroy(() => {
		lifetimeGate.cancel();
		if (openNoteReloadRetry) clearTimeout(openNoteReloadRetry);
		initialLoadGate.cancel();
		unlistenFileChange?.();
		unlistenSyncDone?.();
		unlistenUiScale?.();
		removeNavigationRequest?.();
	});
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="note-window">
	{#if $shutdownPending}
		<div class="shutdown-lock" aria-label="Saving before close" aria-busy="true"></div>
	{/if}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="nw-titlebar" onmousedown={handleMouseDown}>
		<div class="nw-titlebar-brand">
			<svg width="14" height="14" viewBox="0 0 48 48" fill="none">
				<rect width="48" height="48" rx="12" fill="var(--accent)" />
				<circle cx="16" cy="16" r="3.5" fill="white" opacity="0.9" />
				<circle cx="32" cy="16" r="3.5" fill="white" opacity="0.9" />
				<circle cx="16" cy="32" r="3.5" fill="white" opacity="0.9" />
				<circle cx="32" cy="32" r="3.5" fill="white" opacity="0.9" />
				<line x1="19" y1="18" x2="29" y2="30" stroke="white" stroke-width="2" stroke-linecap="round" opacity="0.7" />
				<line x1="29" y1="18" x2="19" y2="30" stroke="white" stroke-width="2" stroke-linecap="round" opacity="0.7" />
			</svg>
			<span class="nw-title">{$activeNote?.meta.title || 'Loading...'}</span>
			{#if $editorDirty}
				<span class="nw-dirty-dot" title="Unsaved changes"></span>
			{/if}
		</div>
		<div class="titlebar-actions">
			<button class="nw-btn" class:active={$sourceMode} onclick={() => { if (!$shutdownPending) $sourceMode = !$sourceMode; }} disabled={$shutdownPending} title="Toggle Source Mode">
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<polyline points="16 18 22 12 16 6" /><polyline points="8 6 2 12 8 18" />
				</svg>
			</button>
			<button class="nw-btn" class:active={$readOnly} onclick={() => { if (!$shutdownPending) $readOnly = !$readOnly; }} disabled={$shutdownPending} title={$readOnly ? 'Switch to Edit Mode' : 'Switch to View Mode'}>
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					{#if $readOnly}
						<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
						<circle cx="12" cy="12" r="3" />
					{:else}
						<path d="M17.94 17.94A10.07 10.07 0 0112 20c-7 0-11-8-11-8a18.45 18.45 0 015.06-5.94" />
						<path d="M9.9 4.24A9.12 9.12 0 0112 4c7 0 11 8 11 8a18.5 18.5 0 01-2.16 3.19" />
						<line x1="1" y1="1" x2="23" y2="23" />
					{/if}
				</svg>
			</button>
		</div>
		<div class="titlebar-controls">
			<button class="titlebar-btn" onclick={() => appWindow.minimize()} title="Minimize">
				<svg width="10" height="10" viewBox="0 0 10 10">
					<line x1="1" y1="5" x2="9" y2="5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
				</svg>
			</button>
			<button class="titlebar-btn" onclick={() => appWindow.toggleMaximize()} title={maximized ? 'Restore' : 'Maximize'}>
				{#if maximized}
					<svg width="10" height="10" viewBox="0 0 10 10">
						<rect x="2.5" y="0.5" width="7" height="7" rx="1" fill="none" stroke="currentColor" stroke-width="1.2" />
						<rect x="0.5" y="2.5" width="7" height="7" rx="1" fill="var(--bg-secondary)" stroke="currentColor" stroke-width="1.2" />
					</svg>
				{:else}
					<svg width="10" height="10" viewBox="0 0 10 10">
						<rect x="1" y="1" width="8" height="8" rx="1" fill="none" stroke="currentColor" stroke-width="1.2" />
					</svg>
				{/if}
			</button>
			<button class="titlebar-btn titlebar-close" onclick={() => appWindow.close()} title="Close">
				<svg width="10" height="10" viewBox="0 0 10 10">
					<line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
					<line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
				</svg>
			</button>
		</div>
	</div>

	{#if loadError}
		<div class="nw-error">
			<p>Failed to load note</p>
			<p class="nw-error-detail">{loadError}</p>
		</div>
	{:else}
		<div class="nw-editor">
			<Editor bind:this={editor} onNavigateNote={navigateToPath} onNavigateWikiNote={navigateToPathResult} onRelocateActiveDocument={relocateInTurn} />
		</div>
	{/if}
</div>

<style>
	.note-window {
		display: flex;
		flex-direction: column;
		height: 100vh;
		background: var(--bg-primary);
	}

	.shutdown-lock {
		position: fixed;
		inset: 0;
		z-index: 2147483647;
		cursor: wait;
	}

	.nw-titlebar {
		display: flex;
		align-items: center;
		height: 34px;
		background: var(--bg-secondary);
		border-bottom: 1px solid var(--border-color);
		user-select: none;
		flex-shrink: 0;
		-webkit-app-region: drag;
	}


	.nw-titlebar-brand {
		display: flex;
		align-items: center;
		gap: 8px;
		padding-left: 14px;
		pointer-events: none;
		flex: 1;
		min-width: 0;
	}

	.nw-title {
		font-size: 12px;
		font-weight: 500;
		color: var(--text-tertiary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.nw-dirty-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--accent);
		flex-shrink: 0;
	}

	.titlebar-actions {
		display: flex;
		align-items: center;
		gap: 4px;
		margin-right: 8px;
		-webkit-app-region: no-drag;
	}

	.nw-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		border: none;
		border-radius: 7px;
		background: var(--bg-hover);
		color: var(--text-secondary);
		cursor: pointer;
		transition: background 0.15s, color 0.15s;
	}

	.nw-btn:hover {
		background: var(--bg-tertiary, var(--bg-hover));
		color: var(--text-primary);
	}

	.nw-btn.active {
		background: color-mix(in srgb, var(--accent) 18%, transparent);
		color: var(--accent);
	}

	.titlebar-controls {
		display: flex;
		height: 100%;
		-webkit-app-region: no-drag;
	}

	.titlebar-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 38px;
		height: 100%;
		border: none;
		background: none;
		color: var(--text-tertiary);
		cursor: pointer;
		transition: background 0.1s, color 0.1s;
	}

	.titlebar-btn:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.titlebar-close:hover {
		background: #e81123;
		color: white;
	}

	.nw-editor {
		flex: 1;
		overflow: hidden;
	}

	.nw-error {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		color: var(--text-secondary);
		font-size: 14px;
	}

	.nw-error-detail {
		font-size: 12px;
		color: var(--text-tertiary);
		max-width: 400px;
		text-align: center;
		word-break: break-word;
	}
</style>
