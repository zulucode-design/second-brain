<script lang="ts">
	import {
		notes,
		sortedNotes,
		activeNotePath,
		activeNote,
		viewMode,
		activeNotebook,
		activeTag,
		sortMode,
		editorDirty,
		quickAccessPaths,
		appConfig,
		notebooks
	} from '$lib/stores/app';
	import {
		getNotes,
		readNote,
		createNote,
		deleteNote,
		renameNote,
		saveNote,
		getTrash,
		restoreNote,
		permanentDelete,
		getQuickAccess,
		addQuickAccess,
		removeQuickAccess,
		reorderQuickAccess,
		moveNote
	} from '$lib/api';
	import { formatRelativeTime } from '$lib/utils/time';
	import type { NoteEntry, SortMode } from '$lib/types';

	let { onNoteSelected = (_path: string, _content: string) => {}, onNoteMoved = () => {} }: {
		onNoteSelected?: (path: string, content: string) => void;
		onNoteMoved?: () => void;
	} = $props();

	let compact = $derived($appConfig?.compact_notes ?? false);
	let contextMenu = $state<{ x: number; y: number; note: NoteEntry } | null>(null);
	let sortMenu = $state<{ x: number; y: number } | null>(null);
	let editingNote = $state<string | null>(null);
	let editValue = $state('');
	let movePickerNote = $state<NoteEntry | null>(null);

	// Multi-select
	let selectedPaths = $state<Set<string>>(new Set());
	let lastClickedPath = $state<string | null>(null);
	let batchMovePicker = $state(false);

	// Quick Access drag-to-reorder
	let qaDragFrom = $state<number | null>(null);
	let qaDragOver = $state<number | null>(null);
	let qaDragHalf = $state<'top' | 'bottom'>('bottom');

	function clearSelection() {
		selectedPaths = new Set();
		lastClickedPath = null;
		batchMovePicker = false;
	}

	// Clear selection when view/notebook changes
	$effect(() => {
		const _ = [$viewMode, $activeNotebook, $activeTag];
		clearSelection();
	});

	interface FlatNotebook { name: string; path: string; depth: number; }

	function flattenNotebooks(nbs: import('$lib/types').NotebookEntry[], depth = 0): FlatNotebook[] {
		let result: FlatNotebook[] = [];
		for (const nb of nbs) {
			result.push({ name: nb.name, path: nb.path, depth });
			if (nb.children.length > 0) {
				result = result.concat(flattenNotebooks(nb.children, depth + 1));
			}
		}
		return result;
	}

	let flatNotebookList = $derived.by(() => {
		if (!movePickerNote && !batchMovePicker) return [];
		const all = flattenNotebooks($notebooks);
		if (movePickerNote) {
			const noteDir = movePickerNote.path.substring(0, movePickerNote.path.lastIndexOf('/'));
			return all.filter(nb => nb.path !== noteDir);
		}
		return all;
	});

	let viewTitle = $derived.by(() => {
		if ($viewMode === 'all') return 'All Notes';
		if ($viewMode === 'notebook') return $activeNotebook?.name ?? 'Notebook';
		if ($viewMode === 'tag') return `#${$activeTag}`;
		if ($viewMode === 'quickaccess') return 'Quick Access';
		if ($viewMode === 'trash') return 'Trash';
		return 'Notes';
	});

	// Note list cache: avoids re-scanning when switching views
	let noteCache = new Map<string, NoteEntry[]>();

	function cacheKey(): string {
		if ($viewMode === 'trash') return 'trash';
		if ($viewMode === 'quickaccess') return 'quickaccess';
		if ($viewMode === 'tag' && $activeTag) return `tag:${$activeTag}`;
		if ($viewMode === 'notebook' && $activeNotebook) return `nb:${$activeNotebook.path}`;
		return 'all';
	}

	export async function refresh(invalidate = false) {
		if (invalidate) {
			noteCache.clear();
		}

		const key = cacheKey();
		const cached = noteCache.get(key);
		if (cached) {
			$notes = cached;
			return;
		}

		try {
			if ($viewMode === 'trash') {
				$notes = await getTrash();
			} else if ($viewMode === 'quickaccess') {
				$notes = await getQuickAccess();
			} else if ($viewMode === 'tag' && $activeTag) {
				const all = await getNotes(null);
				$notes = all.filter((n) => n.meta.tags.includes($activeTag!));
			} else {
				const nbPath = $viewMode === 'notebook' ? $activeNotebook?.path ?? null : null;
				$notes = await getNotes(nbPath);
			}
			noteCache.set(key, $notes);
		} catch (e) {
			console.error('Failed to refresh notes:', e);
		}
	}

	async function selectNote(note: NoteEntry) {
		if ($activeNotePath === note.path) return;
		try {
			const content = await readNote(note.path);
			$activeNote = content;
			$activeNotePath = note.path;
			$editorDirty = false;
			onNoteSelected(note.path, content.content);
		} catch (e) {
			console.error('Failed to read note:', e);
		}
	}

	export async function handleCreateNote() {
		const nbRelative = $viewMode === 'notebook' ? $activeNotebook?.relative_path ?? null : null;
		try {
			const entry = await createNote(nbRelative, 'Untitled');
			noteCache.clear();
			await refresh();
			await selectNote(entry);
		} catch (e) {
			console.error('Failed to create note:', e);
		}
	}

	async function handleRename(note: NoteEntry) {
		if (!editValue.trim() || editValue.trim() === note.meta.title) {
			editingNote = null;
			return;
		}
		try {
			const newTitle = editValue.trim();
			const newPath = await renameNote(note.path, newTitle);
			noteCache.clear();
			editingNote = null;
			// Update local store immediately (avoid full vault re-scan)
			$notes = $notes.map(n =>
				n.path === note.path
					? { ...n, path: newPath, relative_path: n.relative_path.replace(/[^/]+$/, newTitle + '.md'), meta: { ...n.meta, title: newTitle } }
					: n
			);
			if ($activeNotePath === note.path) {
				$activeNotePath = newPath;
				$activeNote = await readNote(newPath);
			}
		} catch (e) {
			console.error('Failed to rename:', e);
		}
	}

	async function handleDelete(note: NoteEntry) {
		contextMenu = null;
		try {
			await deleteNote(note.path);
			noteCache.clear();
			// Remove from local store immediately (avoid full vault re-scan)
			$notes = $notes.filter(n => n.path !== note.path);
			if ($activeNotePath === note.path) {
				$activeNote = null;
				$activeNotePath = null;
			}
		} catch (e) {
			console.error('Failed to delete:', e);
		}
	}

	async function handleRestore(note: NoteEntry) {
		contextMenu = null;
		try {
			await restoreNote(note.path, null);
			noteCache.clear();
			await refresh();
		} catch (e) {
			console.error('Failed to restore:', e);
		}
	}

	async function handlePermanentDelete(note: NoteEntry) {
		contextMenu = null;
		try {
			await permanentDelete(note.path);
			noteCache.clear();
			$notes = $notes.filter(n => n.path !== note.path);
			if ($activeNotePath === note.path) {
				$activeNote = null;
				$activeNotePath = null;
			}
		} catch (e) {
			console.error('Failed to delete permanently:', e);
		}
	}

	async function handleTogglePin(note: NoteEntry) {
		contextMenu = null;
		try {
			const content = await readNote(note.path);
			content.meta.pinned = !content.meta.pinned;
			await saveNote(note.path, content.meta, content.content);
			// Update local store immediately
			$notes = $notes.map(n =>
				n.path === note.path ? { ...n, meta: { ...n.meta, pinned: content.meta.pinned } } : n
			);
			if ($activeNotePath === note.path) {
				$activeNote = content;
			}
		} catch (e) {
			console.error('Failed to toggle pin:', e);
		}
	}

	async function handleAddQuickAccess(note: NoteEntry) {
		contextMenu = null;
		try {
			await addQuickAccess(note.relative_path);
			const qaNotes = await getQuickAccess();
			$quickAccessPaths = qaNotes.map(n => n.relative_path);
		} catch (e) {
			console.error('Failed to add to quick access:', e);
		}
	}

	async function handleRemoveQuickAccess(note: NoteEntry) {
		contextMenu = null;
		try {
			await removeQuickAccess(note.relative_path);
			const qaNotes = await getQuickAccess();
			$quickAccessPaths = qaNotes.map(n => n.relative_path);
			if ($viewMode === 'quickaccess') await refresh(true);
		} catch (e) {
			console.error('Failed to remove from quick access:', e);
		}
	}

	async function handleQaDrop(targetIndex: number) {
		if (qaDragFrom === null) { qaDragFrom = null; qaDragOver = null; return; }
		// Compute the actual insert position based on which half we're hovering
		let insertAt = qaDragHalf === 'bottom' ? targetIndex + 1 : targetIndex;
		if (qaDragFrom === insertAt || qaDragFrom + 1 === insertAt) {
			qaDragFrom = null; qaDragOver = null; return;
		}
		const arr = [...$sortedNotes];
		const [moved] = arr.splice(qaDragFrom, 1);
		if (insertAt > qaDragFrom) insertAt--;
		arr.splice(insertAt, 0, moved);
		$notes = arr;
		qaDragFrom = null;
		qaDragOver = null;
		try {
			await reorderQuickAccess(arr.map(n => n.relative_path));
		} catch (e) {
			console.error('Failed to reorder quick access:', e);
		}
	}

	async function handleMoveNote(note: NoteEntry, destPath: string) {
		contextMenu = null;
		movePickerNote = null;
		try {
			const newPath = await moveNote(note.path, destPath);
			noteCache.clear();
			$notes = $notes.filter(n => n.path !== note.path);
			if ($activeNotePath === note.path) {
				$activeNotePath = newPath;
				$activeNote = await readNote(newPath);
			}
			onNoteMoved();
		} catch (e) {
			console.error('Failed to move note:', e);
		}
	}

	function onContextMenu(e: MouseEvent, note: NoteEntry) {
		e.preventDefault();
		contextMenu = { x: e.clientX, y: e.clientY, note };
	}

	function startRename(note: NoteEntry) {
		contextMenu = null;
		editingNote = note.path;
		editValue = note.meta.title;
	}

	function handleNoteClick(e: MouseEvent, note: NoteEntry) {
		if (e.ctrlKey || e.metaKey) {
			// Toggle individual selection
			const next = new Set(selectedPaths);
			// On first Ctrl+click, also include the currently active note
			if (next.size === 0 && $activeNotePath && $activeNotePath !== note.path) {
				next.add($activeNotePath);
			}
			if (next.has(note.path)) {
				next.delete(note.path);
			} else {
				next.add(note.path);
			}
			selectedPaths = next;
			lastClickedPath = note.path;
			return;
		}
		if (e.shiftKey) {
			// Range select — use lastClickedPath or active note as anchor
			const anchor = lastClickedPath || $activeNotePath;
			if (!anchor) { selectNote(note); return; }
			const paths = $sortedNotes.map(n => n.path);
			const startIdx = paths.indexOf(anchor);
			const endIdx = paths.indexOf(note.path);
			if (startIdx >= 0 && endIdx >= 0) {
				const from = Math.min(startIdx, endIdx);
				const to = Math.max(startIdx, endIdx);
				const next = new Set(selectedPaths);
				for (let i = from; i <= to; i++) {
					next.add(paths[i]);
				}
				selectedPaths = next;
			}
			return;
		}
		// Normal click — clear selection, open note
		clearSelection();
		selectNote(note);
	}

	// Batch operations
	async function handleBatchDelete() {
		const toDelete = new Set(selectedPaths);
		noteCache.clear();
		$notes = $notes.filter(n => !toDelete.has(n.path));
		if ($activeNotePath && toDelete.has($activeNotePath)) {
			$activeNote = null;
			$activeNotePath = null;
		}
		clearSelection();
		await Promise.all([...toDelete].map(p => deleteNote(p).catch(e => console.error('Failed to delete:', p, e))));
		onNoteMoved();
	}

	async function handleBatchMove(destPath: string) {
		const toMove = new Set(selectedPaths);
		noteCache.clear();
		$notes = $notes.filter(n => !toMove.has(n.path));
		if ($activeNotePath && toMove.has($activeNotePath)) {
			$activeNote = null;
			$activeNotePath = null;
		}
		clearSelection();
		await Promise.all([...toMove].map(p => moveNote(p, destPath).catch(e => console.error('Failed to move:', p, e))));
		onNoteMoved();
	}

	async function handleBatchRestore() {
		const toRestore = [...selectedPaths];
		noteCache.clear();
		clearSelection();
		await Promise.all(toRestore.map(p => restoreNote(p, null).catch(e => console.error('Failed to restore:', p, e))));
		await refresh();
		onNoteMoved();
	}

	async function handleBatchPermanentDelete() {
		const toDelete = new Set(selectedPaths);
		noteCache.clear();
		$notes = $notes.filter(n => !toDelete.has(n.path));
		if ($activeNotePath && toDelete.has($activeNotePath)) {
			$activeNote = null;
			$activeNotePath = null;
		}
		clearSelection();
		await Promise.all([...toDelete].map(p => permanentDelete(p).catch(e => console.error('Failed to delete:', p, e))));
	}

	function handleWindowClick() {
		if (contextMenu) { contextMenu = null; movePickerNote = null; }
		if (sortMenu) sortMenu = null;
	}

	function openSortMenu(e: MouseEvent) {
		e.stopPropagation();
		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		sortMenu = { x: rect.left, y: rect.bottom + 4 };
	}

	function setSortMode(mode: SortMode) {
		$sortMode = mode;
		sortMenu = null;
	}
</script>

<svelte:window onclick={handleWindowClick} onkeydown={(e) => {
	if (e.key === 'Escape' && selectedPaths.size > 0) {
		clearSelection();
	}
	if ((e.ctrlKey || e.metaKey) && e.key === 'a' && $sortedNotes.length > 0) {
		// Only select all if focus is not in an input/editor
		const tag = (e.target as HTMLElement)?.tagName;
		if (tag !== 'INPUT' && tag !== 'TEXTAREA' && !(e.target as HTMLElement)?.closest('.ProseMirror')) {
			e.preventDefault();
			selectedPaths = new Set($sortedNotes.map(n => n.path));
		}
	}
}} />

<div class="note-list">
	<div class="list-header">
		<span class="list-title">{viewTitle}</span>
		<div class="list-actions">
			<button class="icon-btn" onclick={openSortMenu} title="Sort: {$sortMode}">
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<line x1="4" y1="6" x2="11" y2="6" /><line x1="4" y1="12" x2="15" y2="12" /><line x1="4" y1="18" x2="20" y2="18" />
				</svg>
			</button>
			{#if $viewMode !== 'trash'}
				<button class="icon-btn" onclick={handleCreateNote} title="New note (Ctrl+N)">
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<line x1="12" y1="5" x2="12" y2="19" /><line x1="5" y1="12" x2="19" y2="12" />
					</svg>
				</button>
			{/if}
		</div>
	</div>

	{#if selectedPaths.size > 0}
		<div class="selection-bar">
			<span class="selection-count">{selectedPaths.size} selected</span>
			<div class="selection-actions">
				{#if $viewMode === 'trash'}
					<button class="selection-action" onclick={handleBatchRestore}>Restore</button>
					<button class="selection-action danger" onclick={handleBatchPermanentDelete}>Delete</button>
				{:else}
					<button class="selection-action" onclick={() => { batchMovePicker = true; }}>Move</button>
					<button class="selection-action danger" onclick={handleBatchDelete}>Delete</button>
				{/if}
				<button class="selection-close" onclick={clearSelection} title="Clear selection">
					<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
				</button>
			</div>
		</div>
	{/if}

	{#if batchMovePicker}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="batch-move-overlay" onclick={() => batchMovePicker = false}>
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="batch-move-picker" onclick={(e) => e.stopPropagation()}>
				<div class="batch-move-header">
					<span>Move {selectedPaths.size} notes to...</span>
					<button class="selection-close" onclick={() => batchMovePicker = false}>
						<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
					</button>
				</div>
				{#if flatNotebookList.length === 0}
					<div class="context-empty">No notebooks available</div>
				{:else}
					<div class="move-picker-list">
						{#each flatNotebookList as nb}
							<button style="padding-left: {12 + nb.depth * 16}px" onclick={() => handleBatchMove(nb.path)}>
								<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
									<path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
								</svg>
								{nb.name}
							</button>
						{/each}
					</div>
				{/if}
			</div>
		</div>
	{/if}

	<div class="list-content">
		{#if $sortedNotes.length === 0}
			<div class="empty-state">
				{#if $viewMode === 'trash'}
					<p>Trash is empty</p>
				{:else}
					<p>No notes yet</p>
					<button class="btn-link" onclick={handleCreateNote}>Create your first note</button>
				{/if}
			</div>
		{/if}

		{#each $sortedNotes as note, noteIndex (note.path)}
			{#if editingNote === note.path}
				<div class="note-item active">
					<input
						type="text"
						class="rename-input"
						bind:value={editValue}
						onkeydown={(e) => {
							if (e.key === 'Enter') handleRename(note);
							if (e.key === 'Escape') editingNote = null;
						}}
						onblur={() => handleRename(note)}
					/>
				</div>
			{:else}
				<button
					class="note-item"
					class:active={$activeNotePath === note.path}
					class:selected={selectedPaths.has(note.path)}
					class:pinned={note.meta.pinned}
					class:compact={compact}
					class:qa-drag-above={$viewMode === 'quickaccess' && qaDragOver === noteIndex && qaDragFrom !== null && qaDragHalf === 'top'}
					class:qa-drag-below={$viewMode === 'quickaccess' && qaDragOver === noteIndex && qaDragFrom !== null && qaDragHalf === 'bottom'}
					onclick={(e) => handleNoteClick(e, note)}
					oncontextmenu={(e) => {
						e.preventDefault();
						// If multi-selected and right-clicking a selected note, show batch menu
						if (selectedPaths.size > 1 && selectedPaths.has(note.path)) {
							contextMenu = { x: e.clientX, y: e.clientY, note };
							return;
						}
						// Otherwise clear selection and show single menu
						if (selectedPaths.size > 0 && !selectedPaths.has(note.path)) {
							clearSelection();
						}
						contextMenu = { x: e.clientX, y: e.clientY, note };
					}}
					draggable="true"
					ondragstart={(e) => {
						if ($viewMode === 'quickaccess') {
							qaDragFrom = noteIndex;
							e.dataTransfer!.setData('text/plain', note.path);
							e.dataTransfer!.effectAllowed = 'move';
							return;
						}
						if (selectedPaths.size > 1 && selectedPaths.has(note.path)) {
							e.dataTransfer!.setData('text/plain', [...selectedPaths].join('\n'));
						} else {
							e.dataTransfer!.setData('text/plain', note.path);
						}
						e.dataTransfer!.effectAllowed = 'move';
					}}
					ondragover={(e) => {
						if ($viewMode === 'quickaccess' && qaDragFrom !== null) {
							e.preventDefault();
							e.dataTransfer!.dropEffect = 'move';
							const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
							qaDragHalf = e.clientY < rect.top + rect.height / 2 ? 'top' : 'bottom';
							qaDragOver = noteIndex;
						}
					}}
					ondragleave={() => {
						if (qaDragOver === noteIndex) qaDragOver = null;
					}}
					ondrop={(e) => {
						if ($viewMode === 'quickaccess' && qaDragFrom !== null) {
							e.preventDefault();
							handleQaDrop(noteIndex);
						}
					}}
					ondragend={() => { qaDragFrom = null; qaDragOver = null; }}
				>
					{#if compact}
						<div class="note-compact-row">
							<span class="note-title">
								{#if note.meta.pinned}
									<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="pin-icon">
										<path d="M12 17v5"/><path d="M9 2h6l-1 7h4l-2 4H8l-2-4h4L9 2z"/>
									</svg>
								{/if}
								{note.meta.title}
							</span>
							<span class="note-date-compact">{formatRelativeTime(note.meta.modified)}</span>
						</div>
					{:else}
						<div class="note-title">
							{#if note.meta.pinned}
								<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="pin-icon">
									<path d="M12 17v5"/><path d="M9 2h6l-1 7h4l-2 4H8l-2-4h4L9 2z"/>
								</svg>
							{/if}
							{note.meta.title}
						</div>
						<div class="note-preview">{note.preview}</div>
						<div class="note-meta">
							<span class="note-date">{formatRelativeTime(note.meta.modified)}</span>
							{#if note.meta.tags.length > 0}
								<span class="note-tags">
									{#each note.meta.tags.slice(0, 3) as tag}
										<span class="mini-tag">#{tag}</span>
									{/each}
								</span>
							{/if}
						</div>
					{/if}
				</button>
			{/if}
		{/each}
	</div>
</div>

{#if contextMenu}
	<div class="context-menu" style="left: {contextMenu.x}px; top: {contextMenu.y}px" onclick={(e) => e.stopPropagation()} role="menu">
		{#if selectedPaths.size > 1 && selectedPaths.has(contextMenu.note.path)}
			<!-- Batch context menu -->
			{#if $viewMode === 'trash'}
				<button onclick={() => { contextMenu = null; handleBatchRestore(); }}>
					Restore {selectedPaths.size} notes
				</button>
				<button class="danger" onclick={() => { contextMenu = null; handleBatchPermanentDelete(); }}>
					Delete {selectedPaths.size} permanently
				</button>
			{:else}
				<button onclick={() => { contextMenu = null; batchMovePicker = true; }}>
					<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
					</svg>
					Move {selectedPaths.size} notes...
				</button>
				<button class="danger" onclick={() => { contextMenu = null; handleBatchDelete(); }}>
					Delete {selectedPaths.size} notes
				</button>
			{/if}
		{:else if $viewMode === 'trash'}
			<button onclick={() => handleRestore(contextMenu!.note)}>Restore</button>
			<button class="danger" onclick={() => handlePermanentDelete(contextMenu!.note)}>Delete Permanently</button>
		{:else if movePickerNote}
			<button class="move-back" onclick={() => movePickerNote = null}>
				<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<polyline points="15 18 9 12 15 6"/>
				</svg>
				Move to...
			</button>
			<div class="context-sep"></div>
			{#if flatNotebookList.length === 0}
				<div class="context-empty">No other notebooks</div>
			{:else}
				<div class="move-picker-list">
					{#each flatNotebookList as nb}
						<button style="padding-left: {12 + nb.depth * 16}px" onclick={() => handleMoveNote(movePickerNote!, nb.path)}>
							<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
							</svg>
							{nb.name}
						</button>
					{/each}
				</div>
			{/if}
		{:else}
			<button onclick={() => handleTogglePin(contextMenu!.note)}>
				<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<path d="M12 17v5"/><path d="M9 2h6l-1 7h4l-2 4H8l-2-4h4L9 2z"/>
				</svg>
				{contextMenu.note.meta.pinned ? 'Unpin Note' : 'Pin Note'}
			</button>
			{#if $quickAccessPaths.includes(contextMenu.note.relative_path)}
				<button onclick={() => handleRemoveQuickAccess(contextMenu!.note)}>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
					</svg>
					Remove from Quick Access
				</button>
			{:else}
				<button onclick={() => handleAddQuickAccess(contextMenu!.note)}>
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
					</svg>
					Add to Quick Access
				</button>
			{/if}
			<button onclick={() => startRename(contextMenu!.note)}>
				<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<path d="M17 3a2.85 2.85 0 114 4L7.5 20.5 2 22l1.5-5.5Z"/><path d="m15 5 4 4"/>
				</svg>
				Rename
			</button>
			<button onclick={() => { movePickerNote = contextMenu!.note; }}>
				<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/>
				</svg>
				Move to...
			</button>
			<button class="danger" onclick={() => handleDelete(contextMenu!.note)}>
				<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>
				</svg>
				Move to Trash
			</button>
		{/if}
	</div>
{/if}

{#if sortMenu}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="sort-menu" style="left: {sortMenu.x}px; top: {sortMenu.y}px" onmousedown={(e) => e.stopPropagation()}>
		<div class="sort-menu-title">Sort by</div>
		<button class:active={$sortMode === 'modified'} onclick={() => setSortMode('modified')}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<circle cx="12" cy="12" r="10" />
				<polyline points="12 6 12 12 16 14" />
			</svg>
			Last Modified
			{#if $sortMode === 'modified'}<span class="sort-check">&#10003;</span>{/if}
		</button>
		<button class:active={$sortMode === 'created'} onclick={() => setSortMode('created')}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<rect x="3" y="4" width="18" height="18" rx="2" ry="2" />
				<line x1="16" y1="2" x2="16" y2="6" />
				<line x1="8" y1="2" x2="8" y2="6" />
				<line x1="3" y1="10" x2="21" y2="10" />
			</svg>
			Date Created
			{#if $sortMode === 'created'}<span class="sort-check">&#10003;</span>{/if}
		</button>
		<button class:active={$sortMode === 'title'} onclick={() => setSortMode('title')}>
			<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<line x1="4" y1="6" x2="20" y2="6" />
				<line x1="4" y1="12" x2="14" y2="12" />
				<line x1="4" y1="18" x2="18" y2="18" />
			</svg>
			Title
			{#if $sortMode === 'title'}<span class="sort-check">&#10003;</span>{/if}
		</button>
	</div>
{/if}

<style>
	.note-list {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: var(--bg-primary);
		border-right: 1px solid var(--border-color);
		user-select: none;
	}

	.list-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 10px 12px;
		border-bottom: 1px solid var(--border-light);
		flex-shrink: 0;
	}

	.list-title {
		font-weight: 600;
		font-size: 14px;
		color: var(--text-primary);
	}

	.list-actions {
		display: flex;
		gap: 2px;
	}

	.icon-btn {
		background: none;
		border: none;
		color: var(--text-tertiary);
		cursor: pointer;
		padding: 4px;
		border-radius: 4px;
		display: flex;
		align-items: center;
	}

	.icon-btn:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.list-content {
		flex: 1;
		overflow-y: auto;
		padding: 4px;
	}

	.note-item {
		display: block;
		width: 100%;
		padding: 10px 12px;
		border: none;
		background: none;
		border-radius: 6px;
		cursor: pointer;
		text-align: left;
		margin-bottom: 1px;
		contain: content;
	}

	.note-item:hover {
		background: var(--bg-hover);
	}

	.note-item.active {
		background: var(--accent-light);
	}

	.note-item.selected {
		background: color-mix(in srgb, var(--accent) 12%, transparent);
	}

	.note-item.selected:hover {
		background: color-mix(in srgb, var(--accent) 18%, transparent);
	}

	.note-item.selected.active {
		background: var(--accent-light);
	}

	/* Selection bar */
	.selection-bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 6px 12px;
		background: color-mix(in srgb, var(--accent) 10%, var(--bg-primary));
		border-bottom: 1px solid var(--border-light);
		flex-shrink: 0;
	}

	.selection-count {
		font-size: 12px;
		font-weight: 600;
		color: var(--text-accent);
	}

	.selection-actions {
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.selection-action {
		background: none;
		border: 1px solid var(--border-color);
		color: var(--text-primary);
		font-size: 11px;
		padding: 3px 10px;
		border-radius: 4px;
		cursor: pointer;
		font-weight: 500;
	}

	.selection-action:hover {
		background: var(--bg-hover);
	}

	.selection-action.danger {
		color: var(--danger);
		border-color: color-mix(in srgb, var(--danger) 30%, var(--border-color));
	}

	.selection-action.danger:hover {
		background: color-mix(in srgb, var(--danger) 10%, transparent);
	}

	.selection-close {
		background: none;
		border: none;
		color: var(--text-tertiary);
		cursor: pointer;
		padding: 4px;
		border-radius: 4px;
		display: flex;
		align-items: center;
	}

	.selection-close:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	/* Batch move overlay */
	.batch-move-overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.3);
		z-index: 900;
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.batch-move-picker {
		background: var(--bg-primary);
		border: 1px solid var(--border-color);
		border-radius: 10px;
		box-shadow: var(--shadow-lg);
		min-width: 260px;
		max-width: 340px;
		max-height: 400px;
		display: flex;
		flex-direction: column;
		overflow: hidden;
	}

	.batch-move-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 10px 12px;
		border-bottom: 1px solid var(--border-light);
		font-size: 13px;
		font-weight: 600;
		color: var(--text-primary);
	}

	.batch-move-picker .move-picker-list {
		flex: 1;
		overflow-y: auto;
		padding: 4px;
	}

	.batch-move-picker .move-picker-list button {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		padding: 7px 12px;
		border: none;
		background: none;
		color: var(--text-primary);
		font-size: 13px;
		cursor: pointer;
		border-radius: 6px;
		text-align: left;
	}

	.batch-move-picker .move-picker-list button:hover {
		background: var(--bg-hover);
	}

	.note-title {
		font-weight: 500;
		font-size: inherit;
		color: var(--text-primary);
		display: flex;
		align-items: center;
		gap: 4px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.note-item.compact {
		padding: 6px 12px;
	}

	.note-compact-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
	}

	.note-compact-row .note-title {
		flex: 1;
		min-width: 0;
	}

	.note-date-compact {
		font-size: 11px;
		color: var(--text-tertiary);
		flex-shrink: 0;
		white-space: nowrap;
	}

	.pin-icon {
		color: var(--text-accent);
		flex-shrink: 0;
	}

	.note-preview {
		font-size: 0.85em;
		color: var(--text-tertiary);
		margin-top: 2px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.note-meta {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-top: 4px;
	}

	.note-date {
		font-size: 0.78em;
		color: var(--text-tertiary);
	}

	.note-tags {
		display: flex;
		gap: 4px;
	}

	.mini-tag {
		font-size: 10px;
		color: var(--text-accent);
		background: var(--accent-light);
		padding: 1px 5px;
		border-radius: 3px;
	}

	.rename-input {
		width: 100%;
		padding: 4px 8px;
		border: 1px solid var(--accent);
		border-radius: 4px;
		background: var(--bg-primary);
		color: var(--text-primary);
		font-size: inherit;
		outline: none;
	}

	.empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		padding: 48px 16px;
		color: var(--text-tertiary);
		font-size: 13px;
	}

	.btn-link {
		background: none;
		border: none;
		color: var(--text-accent);
		cursor: pointer;
		font-size: 13px;
		margin-top: 8px;
	}

	.btn-link:hover {
		text-decoration: underline;
	}

	.context-menu {
		position: fixed;
		background: var(--bg-primary);
		border: 1px solid var(--border-color);
		border-radius: 8px;
		box-shadow: var(--shadow-md);
		padding: 4px;
		z-index: 1000;
		min-width: 160px;
	}

	.context-menu button {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		padding: 6px 12px;
		border: none;
		background: none;
		color: var(--text-primary);
		font-size: 13px;
		cursor: pointer;
		border-radius: 4px;
		text-align: left;
	}

	.context-menu button:hover {
		background: var(--bg-hover);
	}

	.context-menu button.danger {
		color: var(--danger);
	}

	.context-menu button.danger:hover {
		background: color-mix(in srgb, var(--danger) 10%, transparent);
	}

	.context-sep {
		height: 1px;
		background: var(--border-color);
		margin: 4px 8px;
	}

	.context-empty {
		padding: 8px 12px;
		color: var(--text-tertiary);
		font-size: 12px;
		text-align: center;
	}

	.move-back {
		font-weight: 600;
	}

	.move-picker-list {
		max-height: 240px;
		overflow-y: auto;
	}

	.move-picker-list::-webkit-scrollbar {
		width: 4px;
	}

	.move-picker-list::-webkit-scrollbar-thumb {
		background: var(--text-tertiary);
		border-radius: 2px;
	}

	.sort-menu {
		position: fixed;
		background: var(--bg-primary);
		border: 1px solid var(--border-color);
		border-radius: 10px;
		box-shadow: var(--shadow-lg);
		padding: 4px;
		z-index: 1000;
		min-width: 180px;
	}

	.sort-menu-title {
		padding: 6px 12px 4px;
		font-size: 11px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--text-tertiary);
	}

	.sort-menu button {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		padding: 7px 12px;
		border: none;
		background: none;
		color: var(--text-primary);
		font-size: 13px;
		cursor: pointer;
		border-radius: 6px;
		text-align: left;
	}

	.sort-menu button:hover {
		background: var(--bg-hover);
	}

	.sort-menu button.active {
		color: var(--text-accent);
	}

	.sort-check {
		margin-left: auto;
		color: var(--accent);
		font-size: 14px;
	}

	.note-item.qa-drag-above {
		border-top: 2px solid var(--accent);
	}

	.note-item.qa-drag-below {
		border-bottom: 2px solid var(--accent);
	}
</style>
