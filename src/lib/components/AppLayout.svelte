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
		readOnly,
		activeNote,
		activeNotePath,
		activeNotebook,
		editorDirty,
		showInfo,
		showSettings,
		sourceMode,
		mobileView,
		appConfig
	} from '$lib/stores/app';

	const appWindow = getCurrentWindow();
	const isMac = navigator.platform.startsWith('Mac');
	const isMobile = /android|ios/i.test(navigator.userAgent);
	import { loadVaultState, saveVaultState, readNote, createDailyNote, createBackup, getPendingOpenFile } from '$lib/api';
	import { debounce } from '$lib/utils/debounce';
	import { openNoteWindow } from '$lib/utils/window';
	import { get } from 'svelte/store';
	import type { VaultState, FileEvent } from '$lib/types';

	let sidebar: Sidebar;
	let noteList: NoteList;
	let editor: Editor;
	let unlistenFileChange: (() => void) | null = null;
	let unlistenOpenFile: (() => void) | null = null;
	let backupInterval: ReturnType<typeof setInterval> | null = null;
	let navigatingFromHistory = false;
	let noteHistory: string[] = [];
	let noteHistoryIndex = -1;

	function parseFrequencyMs(freq: string): number {
		switch (freq) {
			case '6h': return 6 * 60 * 60 * 1000;
			case '12h': return 12 * 60 * 60 * 1000;
			case '7d': return 7 * 24 * 60 * 60 * 1000;
			case '24h': default: return 24 * 60 * 60 * 1000;
		}
	}

	async function checkScheduledBackup() {
		const config = get(appConfig);
		if (!config?.backup_enabled) return;
		const interval = parseFrequencyMs(config.backup_frequency);
		const last = config.last_backup_time ? new Date(config.last_backup_time).getTime() : 0;
		if (Date.now() - last >= interval) {
			const unlisten = await listen('backup-done', (event: any) => {
				if (event.payload?.success) {
					const cur = get(appConfig);
					if (cur) appConfig.set({ ...cur, last_backup_time: new Date().toISOString() });
				}
				unlisten();
			});
			try { await createBackup(); } catch (_) { unlisten(); }
		}
	}

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
			editor?.flushSave();
			$activeNote = content;
			$activeNotePath = path;
			$editorDirty = false;
			editor?.loadNote(path, content.content);
		}).catch(() => {
			// Note may have been deleted, ignore
		});
	}

	async function handleOpenFile(filePath: string) {
		if (!filePath || !filePath.endsWith('.md')) return;
		const config = get(appConfig);
		const vaultRoot = config?.active_vault;
		if (!vaultRoot || !filePath.startsWith(vaultRoot)) return;
		try {
			const content = await readNote(filePath);
			editor?.flushSave();
			$activeNote = content;
			$activeNotePath = filePath;
			$editorDirty = false;
			editor?.loadNote(filePath, content.content);
		} catch (e) {
			console.error('Failed to open file:', e);
		}
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
		if (isMobile) $mobileView = 'editor';
	}

	function handleViewChanged() {
		noteList?.refresh(true);
		if (isMobile) $mobileView = 'notelist';
	}

	async function createAndFocusNote() {
		if (isMobile && $mobileView !== 'notelist') $mobileView = 'notelist';
		await noteList?.handleCreateNote();
		editor?.focusTitle();
		if (isMobile) $mobileView = 'editor';
	}

	async function handleDailyNote() {
		try {
			const entry = await createDailyNote();
			const content = await readNote(entry.path);
			editor?.flushSave();
			$activeNote = content;
			$activeNotePath = entry.path;
			$editorDirty = false;
			editor?.loadNote(entry.path, content.content);
			noteList?.refresh();
			sidebar?.refresh();
			if (isMobile) $mobileView = 'editor';
		} catch (e) {
			console.error('Failed to create/open daily note:', e);
		}
	}

	function mobileBack() {
		if ($mobileView === 'editor') $mobileView = 'notelist';
		else if ($mobileView === 'notelist') $mobileView = 'sidebar';
	}

	function handleMouseDown(e: MouseEvent) {
		if (e.button === 3) { e.preventDefault(); navigateHistory(-1); }
		if (e.button === 4) { e.preventDefault(); navigateHistory(1); }
	}

	function handleKeydown(e: KeyboardEvent) {
		const mod = e.ctrlKey || e.metaKey;
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
		if (mod && !e.shiftKey && e.key === 'n') {
			e.preventDefault();
			createAndFocusNote();
		}
		if (mod && e.shiftKey && e.key === 'N') {
			e.preventDefault();
		}
		if (mod && e.shiftKey && e.key === 'F') {
			e.preventDefault();
			$showSearch = true;
			return;
		}
		if (mod && !e.shiftKey && e.key === 'f') {
			e.preventDefault();
			if ($activeNotePath) {
				editor?.openNoteSearch();
			} else {
				$showSearch = true;
			}
			return;
		}
		if (mod && e.key === 'p') {
			e.preventDefault();
			$showCommandPalette = true;
		}
		if (mod && e.key === 's') {
			e.preventDefault();
			editor?.forceSave();
		}
		if (mod && e.key === 'k' && !e.shiftKey && $activeNotePath && !$sourceMode) {
			e.preventDefault();
			editor?.addLinkFromToolbar();
		}
		if (mod && e.shiftKey && e.key === 'M') {
			e.preventDefault();
			$sourceMode = !$sourceMode;
		}
		if (mod && e.shiftKey && e.key === 'W') {
			e.preventDefault();
			if ($activeNotePath && $activeNote) {
				openNoteWindow($activeNotePath, $activeNote.meta.title);
			}
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

		// Listen for file open events (from single-instance or OS file associations)
		unlistenOpenFile = await listen<string>('open-file', async (event) => {
			await handleOpenFile(event.payload);
		});

		// Check for pending file from first-launch CLI args
		try {
			const pending = await getPendingOpenFile();
			if (pending) await handleOpenFile(pending);
		} catch (_) {}

		// Scheduled backup: check on startup and every 5 minutes
		checkScheduledBackup();
		backupInterval = setInterval(checkScheduledBackup, 5 * 60 * 1000);
	});

	onDestroy(() => {
		unlistenFileChange?.();
		unlistenOpenFile?.();
		if (backupInterval) clearInterval(backupInterval);
	});
</script>

<svelte:window onkeydown={handleKeydown} onmousedown={isMobile ? undefined : handleMouseDown} />

{#if isMobile}
	<!-- ═══ MOBILE LAYOUT ═══ -->
	<div class="mobile-shell">
		<!-- Mobile Header -->
		<div class="mobile-header">
			{#if $mobileView !== 'sidebar'}
				<button class="mobile-header-btn" onclick={mobileBack}>
					<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<path d="M15 18l-6-6 6-6"/>
					</svg>
				</button>
			{:else}
				<div class="mobile-header-brand">
					<svg width="22" height="22" viewBox="0 0 48 48" fill="none">
						<rect width="48" height="48" rx="12" fill="var(--accent)" />
						<circle cx="16" cy="16" r="3.5" fill="white" opacity="0.9" />
						<circle cx="32" cy="16" r="3.5" fill="white" opacity="0.9" />
						<circle cx="16" cy="32" r="3.5" fill="white" opacity="0.9" />
						<circle cx="32" cy="32" r="3.5" fill="white" opacity="0.9" />
					</svg>
				</div>
			{/if}
			<span class="mobile-header-title">
				{#if $mobileView === 'sidebar'}
					HelixNotes
				{:else if $mobileView === 'notelist'}
					{$activeNotebook?.name || 'All Notes'}
				{:else}
					{$activeNote?.meta.title || 'Untitled'}
				{/if}
			</span>
			<div class="mobile-header-actions">
				{#if $mobileView === 'editor'}
					<button class="mobile-header-btn" class:active={$readOnly} onclick={() => ($readOnly = !$readOnly)}>
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							{#if $readOnly}
								<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
								<circle cx="12" cy="12" r="3" />
							{:else}
								<path d="M11 4H4a2 2 0 00-2 2v14a2 2 0 002 2h14a2 2 0 002-2v-7" />
								<path d="M18.5 2.5a2.121 2.121 0 013 3L12 15l-4 1 1-4 9.5-9.5z" />
							{/if}
						</svg>
					</button>
				{:else}
					<button class="mobile-header-btn" onclick={createAndFocusNote}>
						<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
							<path d="M12 5v14M5 12h14" />
						</svg>
					</button>
				{/if}
				<button class="mobile-header-btn" onclick={() => ($showInfo = true)} title="Info">
					<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<circle cx="12" cy="12" r="10" />
						<line x1="12" y1="16" x2="12" y2="12" />
						<line x1="12" y1="8" x2="12.01" y2="8" />
					</svg>
				</button>
				<button class="mobile-header-btn" onclick={() => ($showSettings = true)} title="Settings">
					<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<circle cx="12" cy="12" r="3" />
						<path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 01-2.83 2.83l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
					</svg>
				</button>
			</div>
		</div>

		<!-- Mobile Content -->
		<div class="mobile-content">
			<div class="mobile-panel" class:active={$mobileView === 'sidebar'}>
				<Sidebar bind:this={sidebar} onViewChanged={handleViewChanged} />
			</div>
			<div class="mobile-panel" class:active={$mobileView === 'notelist'}>
				<NoteList bind:this={noteList} onNoteSelected={handleNoteSelected} onBeforeNoteSwitch={() => editor?.flushSave()} onNoteMoved={() => sidebar?.refresh()} />
			</div>
			<div class="mobile-panel" class:active={$mobileView === 'editor'}>
				<Editor bind:this={editor} />
			</div>
		</div>

		<!-- Mobile Bottom Nav -->
		{#if $mobileView !== 'editor'}
		<div class="mobile-bottom-nav">
			<button class="mobile-nav-item" class:active={$mobileView === 'sidebar'} onclick={() => ($mobileView = 'sidebar')}>
				<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
					<path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" />
				</svg>
				<span>Notebooks</span>
			</button>
			<button class="mobile-nav-item" onclick={() => $showSearch = true}>
				<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
					<circle cx="11" cy="11" r="8" />
					<line x1="21" y1="21" x2="16.65" y2="16.65" />
				</svg>
				<span>Search</span>
			</button>
			<button class="mobile-nav-item" onclick={handleDailyNote}>
				<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
					<rect x="3" y="4" width="18" height="18" rx="2" ry="2" />
					<line x1="16" y1="2" x2="16" y2="6" />
					<line x1="8" y1="2" x2="8" y2="6" />
					<line x1="3" y1="10" x2="21" y2="10" />
				</svg>
				<span>Daily</span>
			</button>
			<button class="mobile-nav-item" onclick={() => ($showSettings = true)}>
				<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
					<circle cx="12" cy="12" r="3" />
					<path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 01-2.83 2.83l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
				</svg>
				<span>Settings</span>
			</button>
		</div>
		{/if}
	</div>
{:else}
	<!-- ═══ DESKTOP LAYOUT ═══ -->
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
					<button class="focus-btn" class:focus-active={$readOnly} onclick={() => ($readOnly = !$readOnly)} title={$readOnly ? 'Switch to Edit Mode' : 'Switch to View Mode'}>
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
			<TitleBar onNewNote={createAndFocusNote} onDailyNote={handleDailyNote} />
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
					<NoteList bind:this={noteList} onNoteSelected={handleNoteSelected} onBeforeNoteSwitch={() => editor?.flushSave()} onNoteMoved={() => sidebar?.refresh()} />
				</div>

				<ResizeHandle onResize={handleNotelistResize} />
			{/if}

			<div class="editor-panel">
				<Editor bind:this={editor} />
			</div>
		</div>
	</div>
{/if}

<SearchPanel />
<CommandPalette />
<SettingsPanel />
<InfoPanel />

<style>
	/* ═══ DESKTOP STYLES ═══ */
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

	/* ═══ MOBILE STYLES ═══ */
	.mobile-shell {
		display: flex;
		flex-direction: column;
		height: 100vh;
		overflow: hidden;
		background: var(--bg-primary);
	}

	.mobile-header {
		display: flex;
		align-items: center;
		height: 52px;
		padding: 0 8px;
		background: var(--bg-secondary);
		border-bottom: 1px solid var(--border-color);
		flex-shrink: 0;
		gap: 4px;
	}

	.mobile-header-brand {
		display: flex;
		align-items: center;
		padding: 0 8px;
	}

	.mobile-header-title {
		flex: 1;
		font-size: 17px;
		font-weight: 600;
		color: var(--text-primary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		padding: 0 4px;
	}

	.mobile-header-actions {
		display: flex;
		align-items: center;
		gap: 2px;
	}

	.mobile-header-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 44px;
		height: 44px;
		border: none;
		background: none;
		color: var(--text-secondary);
		border-radius: 10px;
		cursor: pointer;
	}

	.mobile-header-btn:active {
		background: var(--bg-hover);
	}

	.mobile-header-btn.active {
		color: var(--accent);
	}

	.mobile-content {
		flex: 1;
		overflow: hidden;
		position: relative;
	}

	.mobile-panel {
		position: absolute;
		inset: 0;
		overflow: hidden;
		visibility: hidden;
		pointer-events: none;
		display: flex;
		flex-direction: column;
	}

	.mobile-panel.active {
		visibility: visible;
		pointer-events: auto;
	}

	.mobile-bottom-nav {
		display: flex;
		align-items: center;
		justify-content: space-around;
		height: 56px;
		background: var(--bg-secondary);
		border-top: 1px solid var(--border-color);
		flex-shrink: 0;
		padding-bottom: env(safe-area-inset-bottom);
	}

	.mobile-nav-item {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 2px;
		padding: 6px 16px;
		border: none;
		background: none;
		color: var(--text-tertiary);
		font-size: 10px;
		font-weight: 500;
		cursor: pointer;
		border-radius: 8px;
		transition: color 0.15s;
	}

	.mobile-nav-item:active {
		background: var(--bg-hover);
	}

	.mobile-nav-item.active {
		color: var(--accent);
	}
</style>
