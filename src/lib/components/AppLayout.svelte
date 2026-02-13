<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { listen } from '@tauri-apps/api/event';
	import Sidebar from './Sidebar.svelte';
	import NoteList from './NoteList.svelte';
	import Editor from './Editor.svelte';
	import SearchPanel from './SearchPanel.svelte';
	import CommandPalette from './CommandPalette.svelte';
	import SettingsPanel from './SettingsPanel.svelte';
	import InfoPanel from './InfoPanel.svelte';
	import TitleBar from './TitleBar.svelte';
	import ResizeHandle from './ResizeHandle.svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import {
		sidebarWidth,
		notelistWidth,
		sidebarCollapsed,
		collapsedNotebooks,
		showSearch,
		showCommandPalette,
		theme,
		focusMode,
		activeNote,
		activeNotePath,
		editorDirty,
		showInfo,
		showSettings
	} from '$lib/stores/app';

	const appWindow = getCurrentWindow();
	const isMac = navigator.platform.startsWith('Mac');
	import { loadVaultState, saveVaultState, readNote } from '$lib/api';
	import { debounce } from '$lib/utils/debounce';
	import type { VaultState, FileEvent } from '$lib/types';

	let sidebar: Sidebar;
	let noteList: NoteList;
	let editor: Editor;
	let unlistenFileChange: (() => void) | null = null;
	let navigatingFromHistory = false;
	let noteHistory: string[] = [];
	let noteHistoryIndex = -1;

	// Track note navigation in history stack
	$effect(() => {
		const path = $activeNotePath;
		if (navigatingFromHistory) {
			navigatingFromHistory = false;
			return;
		}
		if (path) {
			// Trim forward history and push
			noteHistory = [...noteHistory.slice(0, noteHistoryIndex + 1), path];
			noteHistoryIndex = noteHistory.length - 1;
		}
	});

	function navigateHistory(direction: -1 | 1) {
		const newIndex = noteHistoryIndex + direction;
		if (newIndex < 0 || newIndex >= noteHistory.length) return;
		const path = noteHistory[newIndex];
		noteHistoryIndex = newIndex;
		navigatingFromHistory = true;
		readNote(path).then((content) => {
			$activeNote = content;
			$activeNotePath = path;
			$editorDirty = false;
			editor?.loadNote(path, content.content);
		}).catch(() => {
			// Note may have been deleted, ignore
		});
	}

	const persistState = debounce(async () => {
		const state: VaultState = {
			last_open_note: null,
			sidebar_width: $sidebarWidth,
			notelist_width: $notelistWidth,
			sidebar_collapsed: $sidebarCollapsed,
			collapsed_notebooks: $collapsedNotebooks
		};
		try {
			await saveVaultState(state);
		} catch (_) {}
	}, 1000);

	function handleSidebarResize(delta: number) {
		$sidebarWidth = Math.max(160, Math.min(400, $sidebarWidth + delta));
		persistState();
	}

	function handleNotelistResize(delta: number) {
		$notelistWidth = Math.max(200, Math.min(500, $notelistWidth + delta));
		persistState();
	}

	function handleNoteSelected(path: string, content: string) {
		editor?.loadNote(path, content);
	}

	function handleViewChanged() {
		noteList?.refresh(true);
	}

	async function createAndFocusNote() {
		await noteList?.handleCreateNote();
		editor?.focusTitle();
	}

	function handleMouseDown(e: MouseEvent) {
		if (e.button === 3) { e.preventDefault(); navigateHistory(-1); }
		if (e.button === 4) { e.preventDefault(); navigateHistory(1); }
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.altKey && e.key === 'ArrowLeft') {
			e.preventDefault();
			navigateHistory(-1);
			return;
		}
		if (e.altKey && e.key === 'ArrowRight') {
			e.preventDefault();
			navigateHistory(1);
			return;
		}
		if (e.ctrlKey && !e.shiftKey && e.key === 'n') {
			e.preventDefault();
			createAndFocusNote();
		}
		if (e.ctrlKey && e.shiftKey && e.key === 'N') {
			e.preventDefault();
		}
		if (e.ctrlKey && e.shiftKey && e.key === 'F') {
			e.preventDefault();
			$showSearch = true;
			return;
		}
		if (e.ctrlKey && !e.shiftKey && e.key === 'f') {
			e.preventDefault();
			if ($activeNotePath) {
				editor?.openNoteSearch();
			} else {
				$showSearch = true;
			}
			return;
		}
		if (e.ctrlKey && e.key === 'p') {
			e.preventDefault();
			$showCommandPalette = true;
		}
		if (e.ctrlKey && e.key === 's') {
			e.preventDefault();
			editor?.forceSave();
		}
		if (e.key === 'Escape') {
			if ($showSettings) $showSettings = false;
			else if ($showInfo) $showInfo = false;
			else if ($focusMode) $focusMode = false;
			else if ($showSearch) $showSearch = false;
			else if ($showCommandPalette) $showCommandPalette = false;
		}
	}

	function applyTheme(t: string) {
		const root = document.documentElement;
		root.classList.remove('dark');
		if (t === 'dark' || (t === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)) {
			root.classList.add('dark');
		}
	}

	$effect(() => {
		applyTheme($theme);
	});

	$effect(() => {
		$collapsedNotebooks;
		persistState();
	});

	onMount(async () => {
		try {
			const state = await loadVaultState();
			$sidebarWidth = state.sidebar_width;
			$notelistWidth = state.notelist_width;
			$sidebarCollapsed = state.sidebar_collapsed;
			$collapsedNotebooks = state.collapsed_notebooks ?? [];
		} catch (_) {}

		applyTheme($theme);

		window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
			if ($theme === 'system') applyTheme('system');
		});

		await sidebar?.refresh();
		await noteList?.refresh();

		unlistenFileChange = await listen<FileEvent>('file-changed', async () => {
			await sidebar?.refresh();
			await noteList?.refresh(true);
		});
	});

	onDestroy(() => {
		unlistenFileChange?.();
	});
</script>

<svelte:window onkeydown={handleKeydown} onmousedown={handleMouseDown} />

<div class="app-shell">
	{#if $focusMode}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="focus-topbar" class:macos={isMac} onmousedown={(e) => { if (!(e.target as HTMLElement).closest('button')) appWindow.startDragging(); }}>
			<span class="focus-title">{$activeNote?.meta.title || 'Untitled'}</span>
			<div class="focus-controls">
				<button class="focus-btn focus-active" onclick={() => ($focusMode = false)} title="Exit focus mode (Escape)">
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<path d="M8 3v3a2 2 0 01-2 2H3m18 0h-3a2 2 0 01-2-2V3m0 18v-3a2 2 0 012-2h3M3 16h3a2 2 0 012 2v3"/>
					</svg>
				</button>
				{#if !isMac}
				<button class="focus-btn" onmousedown={(e) => e.stopPropagation()} onclick={() => appWindow.minimize()} title="Minimize">
					<svg width="10" height="10" viewBox="0 0 10 10"><line x1="1" y1="5" x2="9" y2="5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
				</button>
				<button class="focus-btn" onmousedown={(e) => e.stopPropagation()} onclick={() => appWindow.toggleMaximize()} title="Maximize">
					<svg width="10" height="10" viewBox="0 0 10 10"><rect x="1" y="1" width="8" height="8" rx="1" fill="none" stroke="currentColor" stroke-width="1.2"/></svg>
				</button>
				<button class="focus-btn focus-close" onmousedown={(e) => e.stopPropagation()} onclick={() => appWindow.close()} title="Close">
					<svg width="10" height="10" viewBox="0 0 10 10"><line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
				</button>
				{/if}
			</div>
		</div>
	{:else}
		<TitleBar onNewNote={createAndFocusNote} />
	{/if}
	<div class="app-layout">
		{#if !$focusMode}
			<div class="sidebar-panel" style="width: {$sidebarCollapsed ? 44 : $sidebarWidth}px">
				<Sidebar bind:this={sidebar} onViewChanged={handleViewChanged} />
			</div>

			{#if !$sidebarCollapsed}
				<ResizeHandle onResize={handleSidebarResize} />
			{/if}

			<div class="notelist-panel" style="width: {$notelistWidth}px">
				<NoteList bind:this={noteList} onNoteSelected={handleNoteSelected} onNoteMoved={() => sidebar?.refresh()} />
			</div>

			<ResizeHandle onResize={handleNotelistResize} />
		{/if}

		<div class="editor-panel">
			<Editor bind:this={editor} />
		</div>
	</div>
</div>

<SearchPanel />
<CommandPalette />
<SettingsPanel />
<InfoPanel />

<style>
	.app-shell {
		display: flex;
		flex-direction: column;
		height: 100vh;
		overflow: hidden;
	}

	.app-layout {
		display: flex;
		flex: 1;
		overflow: hidden;
	}

	.sidebar-panel {
		flex-shrink: 0;
		height: 100%;
		overflow: hidden;
	}

	.notelist-panel {
		flex-shrink: 0;
		height: 100%;
		overflow: hidden;
	}

	.editor-panel {
		flex: 1;
		height: 100%;
		overflow: hidden;
		min-width: 300px;
	}

	.focus-topbar {
		display: flex;
		align-items: center;
		height: 34px;
		background: var(--bg-secondary);
		border-bottom: 1px solid var(--border-light);
		user-select: none;
		flex-shrink: 0;
		padding: 0 8px 0 16px;
	}

	.focus-title {
		flex: 1;
		font-size: 12px;
		color: var(--text-tertiary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		pointer-events: none;
	}

	.focus-controls {
		display: flex;
		align-items: center;
		gap: 2px;
	}

	.focus-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 32px;
		height: 26px;
		border: none;
		background: none;
		color: var(--text-tertiary);
		cursor: pointer;
		border-radius: 4px;
		transition: background 0.1s, color 0.1s;
	}

	.focus-btn.focus-active {
		color: var(--text-accent);
		background: var(--accent-light);
	}

	.focus-btn:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.focus-close:hover {
		background: #e81123;
		color: white;
	}

	.focus-topbar.macos {
		padding-left: 78px;
	}
</style>
