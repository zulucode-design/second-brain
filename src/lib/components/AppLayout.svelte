<script lang="ts">
	import { installEditorKeyProbe, markStartupReady } from '$lib/perf-probe';
	import { onMount, onDestroy, tick, untrack } from 'svelte';
	import { listenAppEvent } from '$lib/events';
	import Sidebar from './Sidebar.svelte';
	import NoteList from './NoteList.svelte';
	import Editor from './Editor.svelte';
	import SearchPanel from './SearchPanel.svelte';
	import SimilarCards from './SimilarCards.svelte';
	import CommandPalette from './CommandPalette.svelte';
	import SettingsPanel from './SettingsPanel.svelte';
	import InfoPanel from './InfoPanel.svelte';
	import TitleBar from './TitleBar.svelte';
	import ResizeHandle from './ResizeHandle.svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import {
		sidebarWidth,
		notelistWidth,
		outlineWidth,
		sidebarCollapsed,
		notelistCollapsed,
		collapsedNotebooks,
		showSearch,
		showCommandPalette,
		theme,
		resolvedTheme,
		customThemes,
		focusMode,
		readOnly,
		shutdownPending,
		vaultReady,
		holdingPreview,
		activeNote,
		activeNotePath,
		activeNotebook,
		editorDirty,
		showInfo,
		showSettings,
		sourceMode,
		compactView,
		appConfig,
		quickAccessPaths,
		notes,
		notebooks,
		rootNoteCount,
		viewMode,
		sortMode,
		groupNotesByDate,
		tasksLayout,
		tasksHideCompleted,
		tasksOnlyFlagged,
		tasksSort,
		activeTag,
		navHistory,
		viewerNote,
		notebookSortMode,
		notebookOrder,
		noteOrder,
		compactLayout,
		unfiledNotes,
		aiStatus,
		hotkeyStatus,
	} from '$lib/stores/app';
	import { keybindings, matchAction } from '$lib/keybindings';
	import { destinationForCategory, suggestedNotebookForCreation } from '$lib/utils/note-creation';
	import {
		canSubmitWebClip,
		cleanClipUrlInput,
		webClipFailureMessage
	} from '$lib/utils/web-clipping';
	import { PARA_CATEGORIES } from '$lib/types';

	const appWindow = getCurrentWindow();
	import { loadVaultState, saveVaultState, readNote, readExternalNote, readUnfiledNote, deleteNote, createBackup, getPendingOpenFile, addQuickAccess, removeQuickAccess, getQuickAccess, setTheme, notionStatus, notionPublishNow, setTaskDone, setTaskPriority, setTaskDue, findOrphanedAttachments, trashOrphanedAttachments, listUnfiledNotes, getAiStatus, getHotkeyStatus, getRepairStatus, retryRepairs, dismissRestoreNotice, clipWebPage, beginVaultSwitch, endVaultSwitch } from '$lib/api';
	import { isDarkTheme } from '$lib/theme';
	import { debounce } from '$lib/utils/debounce';
	import { openNoteWindow, closeSecondaryWindowsForVaultSwitch } from '$lib/utils/window';
	import { normalizeStartupView, resolveStartupTarget } from '$lib/utils/startup-view';
	import { NAVIGATE_NOTE_EVENT, isExpectedNote, type NavigateNoteRequest, type NoteNavigationResult } from '$lib/utils/navigation';
	import { relocateReported, reportSaveFailure as reportSaveResult, runSaveGatedAction } from '$lib/utils/document-lifecycle';
	import { GenerationGate } from '$lib/utils/generation-gate';
	import { runActiveDocumentMutation } from '$lib/utils/document-mutation';
	import { describeLoadFailure } from '$lib/utils/async-view-state';
	import { isExternalNotePath } from '$lib/utils/paths';
	import { lockAfterReload, reloadCleanDocument, RELOAD_RETRY_MS } from '$lib/utils/document-reload';
	import { showToast } from '$lib/utils/toast';
	import { get } from 'svelte/store';
	import { repairBanner } from '$lib/utils/repair-banner';
	import type { VaultState, FileEvent, NotebookEntry, TaskItem, AiStatus, HotkeyStatus, RepairStatus, ParaCategory, NoteContent, BulkMutationTerminal } from '$lib/types';
	import type { StartupTarget } from '$lib/utils/startup-view';
	const isCompact = $derived($compactLayout);
	// Narrowing the window while a note is open keeps that note in view.
	$effect(() => {
		if (isCompact && untrack(() => $activeNotePath)) $compactView = 'editor';
	});

	function findNotebookByPath(list: NotebookEntry[], relPath: string): NotebookEntry | null {
		for (const nb of list) {
			if (nb.relative_path === relPath) return nb;
			const found = findNotebookByPath(nb.children, relPath);
			if (found) return found;
		}
		return null;
	}

	let sidebar = $state<Sidebar>();
	let noteList = $state<NoteList>();
	let editor = $state<Editor>();
	let unlistenFileChange: (() => void) | null = null;
	let unlistenSyncDone: (() => void) | null = null;
	let unlistenAiStatus: (() => void) | null = null;
	let unlistenHotkeyStatus: (() => void) | null = null;
	let unlistenRepairStatus: (() => void) | null = null;
	let repairStatus = $state<RepairStatus>({ issues: [] });
	let repairBusy = $state(false);
	let repairError = $state('');
	const banner = $derived(repairBanner(repairStatus, repairError));
	let noteCreationOpen = $state(false);
	let noteCreationBusy = $state(false);
	let suggestedCreationNotebook = $state<string | null>(null);
	let creationDialog = $state<HTMLDivElement>();
	let noteCreationTitle = $state('Untitled');
	let noteCreationSource = $state<'list' | 'wiki-link'>('list');
	let noteCreationError = $state('');
	let webClipOpen = $state(false);
	let webClipBusy = $state(false);
	let webClipUrl = $state('');
	let webClipError = $state('');
	let suggestedWebClipNotebook = $state<string | null>(null);
	let webClipDialog = $state<HTMLDivElement>();
	let webClipUrlInput = $state<HTMLInputElement>();

	async function dismissNotice(key: string) {
		repairBusy = true;
		try {
			repairStatus = await dismissRestoreNotice(key);
		} catch (error) {
			showToast(`Could not dismiss the notice: ${error}`);
		} finally {
			repairBusy = false;
		}
	}

	async function repairNow() {
		repairBusy = true;
		repairError = '';
		try {
			repairStatus = await retryRepairs();
		} catch (error) {
			repairError = String(error);
		} finally {
			repairBusy = false;
		}
	}
	function applyStartupTarget(target: StartupTarget): boolean {
		if (target.mode === 'notebook') {
			const vault = $appConfig?.active_vault;
			const notebook = target.notebookPath === '' && vault
				? { name: 'Unfiled Notes', path: vault, relative_path: '', children: [], note_count: $rootNoteCount }
				: findNotebookByPath($notebooks, target.notebookPath);
			if (!notebook) return false;
			$viewMode = 'notebook';
			$activeNotebook = notebook;
			$activeTag = null;
			return true;
		}

		if (target.mode === 'tag') {
			$viewMode = 'tag';
			$activeTag = target.tag;
			$activeNotebook = null;
			return true;
		}

		$viewMode = target.mode;
		$activeNotebook = null;
		$activeTag = null;
		return true;
	}

	let unlistenOpenFile: (() => void) | null = null;
	let closingRequestId: string | null = null;
	let releaseCloseMutationLock: (() => void) | null = null;
	let readOnlyBeforeClose = false;
	let removeNavigationRequest: (() => void) | null = null;

	// Compact editor header helpers
	let noteRelativePath = $derived($activeNotePath && $appConfig?.active_vault ? $activeNotePath.replace($appConfig.active_vault + '/', '') : '');
	let isQuickAccess = $derived(noteRelativePath ? $quickAccessPaths.includes(noteRelativePath) : false);
	let backupInterval: ReturnType<typeof setInterval> | null = null;
	let notionInterval: ReturnType<typeof setInterval> | null = null;
	let orphanScanTimer: ReturnType<typeof setTimeout> | null = null;
	const startupGate = new GenerationGate();
	const lifetimeGate = new GenerationGate();
	let ownsVaultSwitchGate = false;

	async function releaseOwnedVaultSwitchGate() {
		if (!ownsVaultSwitchGate) return;
		ownsVaultSwitchGate = false;
		try {
			await endVaultSwitch();
		} catch (error) {
			console.error('Could not release vault-switch gate:', error);
		}
	}



	function parseFrequencyMs(freq: string): number {
		switch (freq) {
			case '6h': return 6 * 60 * 60 * 1000;
			case '12h': return 12 * 60 * 60 * 1000;
			case '7d': return 7 * 24 * 60 * 60 * 1000;
			case '24h': default: return 24 * 60 * 60 * 1000;
		}
	}

	async function checkScheduledBackup() {
		const lifetime = lifetimeGate.capture();
		const config = get(appConfig);
		if (!config?.backup_enabled) return;
		const interval = parseFrequencyMs(config.backup_frequency);
		const last = config.last_backup_time ? new Date(config.last_backup_time).getTime() : 0;
		if (Date.now() - last >= interval) {
			const unlisten = await listenAppEvent('backupDone', (event) => {
				if (lifetimeGate.isCurrent(lifetime) && event.payload?.success) {
					const cur = get(appConfig);
					if (cur) appConfig.set({ ...cur, last_backup_time: new Date().toISOString() });
				}
				unlisten();
			});
			if (!lifetimeGate.isCurrent(lifetime)) {
				unlisten();
				return;
			}
			try { await createBackup(); } catch (_) { unlisten(); }
		}
	}

	// Publish to Notion on a timer. A read-only view, fed separately from sync (ADR-0002).
	//
	// Cheap by construction: a push that finds nothing changed makes no API call and opens no
	// note, so ticking every few minutes costs a directory walk. The backend refuses to run
	// two pushes at once, and refuses outright on a machine where Notion is not switched on
	// — which is how exactly one machine ends up publishing.
	let notionPollMs = 5 * 60 * 1000;
	let lastNotionPublish = 0;

	async function checkScheduledNotion() {
		if (Date.now() - lastNotionPublish < notionPollMs) return;
		lastNotionPublish = Date.now();
		// Not connected on this machine is the normal case for every machine but one.
		try { await notionPublishNow(); } catch (_) {}
	}

	// Track note navigation in history stack
	$effect(() => {
		const path = $activeNotePath;
		if (path && !$holdingPreview) navHistory.push(path);
		if (path) startupGate.invalidate();
	});

	$effect(() => {
		if ($editorDirty) startupGate.invalidate();
	});

	let navigationQueue: Promise<void> = Promise.resolve();
	// The open-note reload running now; a close waits it out before taking the editor lock.
	let openNoteReload: Promise<unknown> = Promise.resolve();
	let openNoteReloadRetry: ReturnType<typeof setTimeout> | null = null;

	// A conflict choice or a sync run can rewrite the open note. A clean editor then shows the
	// disk's version, so its next save, and the save before close, expect the right revision
	// (#191). Unsaved edits are never replaced; a draft or an open editor dialog delays it.
	function reloadOpenNoteFromDisk(): Promise<void> {
		const lifetime = lifetimeGate.capture();
		const run = navigationQueue.then(() => {
			const current = editor;
			if (!current || !lifetimeGate.isCurrent(lifetime)) return 'skipped' as const;
			const reloadable = () => lifetimeGate.isCurrent(lifetime) && editor === current
				&& !$shutdownPending && !$viewerNote && !$holdingPreview && !$editorDirty;
			const reload = reloadCleanDocument({
				capture: () => reloadable() && $activeNotePath ? { path: $activeNotePath, revision: current.getLoadedRevision() } : null,
				drafting: () => current.hasPendingDraft(),
				read: readNote,
				lock: () => current.lockMutations(),
				stillValid: (captured) => reloadable() && $activeNotePath === captured.path && current.getLoadedRevision() === captured.revision,
				commit: (path, content) => {
					$activeNote = content;
					current.loadNote(path, content.content, undefined, false, content.revision, true);
				},
			});
			openNoteReload = reload;
			return reload;
		});
		navigationQueue = run.then(() => {}, () => {});
		return run.then((outcome) => {
			if (outcome !== 'deferred' || openNoteReloadRetry || !lifetimeGate.isCurrent(lifetime)) return;
			openNoteReloadRetry = setTimeout(() => {
				openNoteReloadRetry = null;
				if (lifetimeGate.isCurrent(lifetime)) void reloadOpenNoteFromDisk();
			}, RELOAD_RETRY_MS);
		}, (error) => console.error('Failed to reload the open note:', error));
	}

	async function ensureCurrentNoteSaved(reason: string): Promise<boolean> {
		let release: (() => void) | null = null;
		try {
			release = editor ? await editor.lockMutations() : null;
			return reportSaveResult(reason, await editor?.flushSave());
		} catch (error) {
			return reportSaveResult(reason, { ok: false, status: 'failed', revision: 0, error });
		} finally {
			release?.();
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
				return reportSaveResult('Closing the application', { ok: false, status: 'failed', revision: 0, error });
			}
		}
		return reportSaveResult('Closing the application', await editor?.flushSave());
	}

	export function releaseClose(requestId: string) {
		if (closingRequestId !== requestId) return;
		releaseCloseMutationLock?.();
		releaseCloseMutationLock = null;
		$readOnly = readOnlyBeforeClose;
		$shutdownPending = false;
		closingRequestId = null;
	}

	function afterCurrentNoteSaved(reason: string, action: () => Promise<boolean>): Promise<boolean> {
		if ($shutdownPending) return Promise.resolve(false);
		const run = navigationQueue.then(async () => {
			if ($shutdownPending) return false;
			return runSaveGatedAction(
				() => ensureCurrentNoteSaved(reason),
				async () => $shutdownPending ? false : action(),
			);
		});
		navigationQueue = run.then(() => {}, () => {});
		return run;
	}

	async function relocateActiveDocument(
		expectedPath: string,
		reason: string,
		mutation: () => Promise<import('$lib/types').RelocationOutcome>,
	): Promise<string | null> {
		if ($shutdownPending) return null;
		const run = navigationQueue.then(async (): Promise<string | null> => {
			if ($shutdownPending || !editor) return null;
			return relocateReported({
				expectedPath,
				reason,
				report: showToast,
				currentPath: () => $activeNotePath,
				prepare: () => editor!.lockMutations(),
				flush: async () => {
					const result = await editor!.flushSave();
					await reportSaveResult(reason, result);
					return result;
				},
				mutate: mutation,
				rebase: (oldPath, newPath, content) => editor!.rebaseDocument(oldPath, newPath, content),
			});
		});
		navigationQueue = run.then(() => {}, () => {});
		return run;
	}

	async function updateActiveMetadata(
		expectedPath: string,
		patch: Partial<NoteContent['meta']>,
		reason: string,
	): Promise<boolean> {
		if ($shutdownPending) return false;
		const run = navigationQueue.then(async (): Promise<boolean> => {
			if ($shutdownPending || !editor || $activeNotePath !== expectedPath) return false;
			let release: (() => void) | null = null;
			try {
				release = await editor.lockMutations();
				if (!(await reportSaveResult(reason, await editor.flushSave()))) return false;
				if ($shutdownPending || $activeNotePath !== expectedPath) return false;
				return reportSaveResult(reason, await editor.updateMetadata(expectedPath, patch));
			} catch (error) {
				return reportSaveResult(reason, { ok: false, status: 'failed', revision: 0, error });
			} finally {
				release?.();
			}
		});
		navigationQueue = run.then(() => {}, () => {});
		return run;
	}

	export async function requestVaultSwitch(): Promise<boolean> {
		if ($shutdownPending) return false;
		try {
			await beginVaultSwitch();
			ownsVaultSwitchGate = true;
		} catch (error) {
			console.error('Could not begin vault switch:', error);
			return false;
		}

		const previousReadOnly = $readOnly;
		const run = navigationQueue.then(async (): Promise<boolean> => {
			if ($shutdownPending) return false;
			let release: (() => void) | null = null;
			try {
				// Keep the main editor immutable from its first save through secondary-window
				// shutdown and the final save immediately before teardown.
				$readOnly = true;
				await tick();
				release = editor ? await editor.lockMutations() : null;
				if (!(await reportSaveResult('Switching vaults', await editor?.flushSave()))) return false;
				if (!(await closeSecondaryWindowsForVaultSwitch())) {
					console.error('Vault switch cancelled because a secondary note window did not save and close.');
					return false;
				}
				if (!(await reportSaveResult('Switching vaults', await editor?.flushSave()))) return false;
				$showSettings = false;
				// VaultPicker now owns the gate and releases it after open, cancellation, or teardown.
				ownsVaultSwitchGate = false;
				$vaultReady = false;
				return true;
			} catch (error) {
				console.error('Vault switch failed:', error);
				return false;
			} finally {
				release?.();
				$readOnly = previousReadOnly;
			}
		});
		navigationQueue = run.then(() => {}, () => {});
		const switched = await run;
		if (!switched) await releaseOwnedVaultSwitchGate();
		return switched;
	}

	function commitNote(path: string, content: Awaited<ReturnType<typeof readNote>>, task?: TaskItem, holding = false): boolean {
		if ($shutdownPending) return false;
		$viewerNote = null;
		$activeNote = content;
		$activeNotePath = path;
		handleNoteSelected(path, content, task, holding);
		return true;
	}

	async function navigateToPathResult(path: string, task?: TaskItem, holding = false, expectedId?: string | null): Promise<NoteNavigationResult> {
		if (!path || $shutdownPending) return 'blocked';
		if ($activeNotePath === path && !$viewerNote) {
			if ($activeNote && !isExpectedNote($activeNote, expectedId)) return 'not-found';
			if (isCompact) $compactView = 'editor';
			return 'navigated';
		}

		const run = navigationQueue.then(async (): Promise<NoteNavigationResult> => {
			if ($shutdownPending) return 'blocked';
			if (!(await ensureCurrentNoteSaved('Navigation'))) return 'save-failed';
			if ($shutdownPending) return 'blocked';

			let content: Awaited<ReturnType<typeof readNote>>;
			try {
				content = holding ? await readUnfiledNote(path) : await readNote(path);
			} catch (error) {
				console.error('Failed to navigate to note:', error);
				return 'not-found';
			}
			if (!isExpectedNote(content, expectedId)) return 'not-found';

			// The destination read yielded to the event loop; drain any edit made in that
			// interval before synchronously replacing the document.
			if ($shutdownPending) return 'blocked';
			if (!(await ensureCurrentNoteSaved('Navigation'))) return 'save-failed';
			if ($shutdownPending) return 'blocked';
			return commitNote(path, content, task, holding) ? 'navigated' : 'blocked';
		});
		navigationQueue = run.then(() => {}, () => {});
		return run;
	}

	async function navigateToPath(path: string, task?: TaskItem, holding = false): Promise<boolean> {
		return (await navigateToPathResult(path, task, holding)) === 'navigated';
	}

	async function navigateHistory(direction: -1 | 1): Promise<boolean> {
		return afterCurrentNoteSaved('History navigation', async () => {
			// Resolve the directional intent only when its serialized turn begins so rapid
			// Back/Forward requests cannot act on a stale cursor.
			const history = get(navHistory);
			const path = history.stack[history.index + direction];
			if (!path) return false;
			try {
				const content = await readNote(path);
				if ($shutdownPending || !(await ensureCurrentNoteSaved('History navigation')) || $shutdownPending) return false;
				if (navHistory.go(direction) !== path) return false;
				return commitNote(path, content);
			} catch (error) {
				console.error('Failed to navigate history:', error);
				return false;
			}
		});
	}

	async function handleOpenFile(filePath: string) {
		// Case-insensitive: a Windows file association can hand back `NOTE.MD`.
		if (!filePath || !filePath.toLowerCase().endsWith('.md')) return;
		const config = get(appConfig);
		const vaultRoot = config?.active_vault;
		const isExternal = isExternalNotePath(vaultRoot, filePath);

		if (isExternal) {
			await afterCurrentNoteSaved('Opening the file', async () => {
				try {
					const content = await readExternalNote(filePath);
					if ($shutdownPending || !(await ensureCurrentNoteSaved('Opening the file')) || $shutdownPending) return false;
					$viewerNote = { path: filePath, content: content.content };
					$activeNote = content;
					$activeNotePath = filePath;
					$readOnly = true;
					$focusMode = true;
					editor?.loadNote(filePath, content.content, undefined, false, content.revision);
					return true;
				} catch (error) {
					console.error('Failed to open external file:', error);
					showToast(`Could not open this file: ${describeLoadFailure(error)}`);
					return false;
				}
			});
			return;
		}

		await navigateToPath(filePath);
	}

	// Startup restoration reads the saved state and then reopens the last note. Persisting
	// before it finishes wrote `last_open_note: null` whenever the note took longer than the
	// debounce to load, so the next launch had nothing to restore (#128).
	let restorationSettled = false;

	const persistState = debounce(async () => {
		if (!restorationSettled) return;
		const state: VaultState = {
			last_open_note: $activeNotePath,
			sidebar_width: $sidebarWidth,
			notelist_width: $notelistWidth,
			outline_width: $outlineWidth,
			sidebar_collapsed: $sidebarCollapsed,
			notelist_collapsed: $notelistCollapsed,
			collapsed_notebooks: $collapsedNotebooks,
			notebook_sort_mode: $notebookSortMode,
			notebook_order: $notebookOrder,
			note_order: $noteOrder,
			sort_mode: $sortMode,
			group_notes_by_date: $groupNotesByDate,
			last_view_mode: $viewMode,
			last_notebook: $activeNotebook?.relative_path ?? null,
			last_tag: $activeTag,
			tasks_layout: $tasksLayout,
			tasks_hide_completed: $tasksHideCompleted,
			tasks_only_flagged: $tasksOnlyFlagged,
			tasks_sort: $tasksSort
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

	function revealNoteList() {
		$notelistCollapsed = false;
		tick().then(() => noteList?.refresh());
	}

	function toggleNoteList() {
		const nextCollapsed = !$notelistCollapsed;
		$notelistCollapsed = nextCollapsed;
		if (!nextCollapsed) {
			tick().then(() => noteList?.refresh());
		}
	}

	// Tasks view: the editor pane shows a placeholder until a task is opened from the list.
	let taskNoteOpened = $state(false);

	function handleNoteSelected(path: string, content: NoteContent, task?: TaskItem, holding = false) {
		// Selecting a real vault note exits viewer mode
		$viewerNote = null;
		taskNoteOpened = true;
		editor?.loadNote(path, content.content, task, holding, content.revision);
		if (isCompact) $compactView = 'editor';
	}

	async function selectNoteFromSwitcher(path: string): Promise<boolean> {
		return navigateToPath(path);
	}

	async function openNoteInSecondaryWindow(path: string, title: string): Promise<boolean> {
		return afterCurrentNoteSaved('Opening a secondary window', async () => {
			try {
				await openNoteWindow(path, title);
				return true;
			} catch (error) {
				console.error('Failed to open note window:', error);
				return false;
			}
		});
	}

	function handleViewChanged() {
		taskNoteOpened = false;
		// Picking a notebook/tag/Tasks/etc. in the sidebar is a request to browse that view's
		// notes - so if the notes list was hidden, bring it back rather than navigating blindly
		// into a panel the user can't see. revealNoteList() re-mounts the list and refreshes it.
		if ($notelistCollapsed) {
			revealNoteList();
		} else {
			noteList?.refresh();
		}
		if (isCompact) $compactView = 'notelist';
	}

	async function prepareForRestore(): Promise<boolean> {
		if ($activeNotePath && !(await editor?.forceSave())) return false;
		editor?.flushSave();
		$activeNote = null;
		$activeNotePath = null;
		$editorDirty = false;
		return true;
	}

	async function refreshAfterRestore(): Promise<void> {
		await Promise.all([sidebar?.refresh(), noteList?.refresh(true)]);
	}

	async function refreshAfterSync(): Promise<void> {
		// The run suppressed the watcher, so nothing else reloads a note it rewrote.
		void reloadOpenNoteFromDisk();
		await Promise.all([sidebar?.refresh(), noteList?.refresh(true), refreshUnfiled()]);
	}

	function requestNoteCreation() {
		if ($shutdownPending || $viewMode === 'quickaccess' || $viewMode === 'trash' || $viewMode === 'unfiled') return;
		if (!isCompact && $notelistCollapsed) $notelistCollapsed = false;
		noteCreationTitle = 'Untitled';
		noteCreationSource = 'list';
		openNoteCreationDialog();
	}

	function requestLinkedNoteCreation(title: string) {
		if ($shutdownPending) return;
		noteCreationTitle = title;
		noteCreationSource = 'wiki-link';
		openNoteCreationDialog();
	}

	function openNoteCreationDialog() {
		suggestedCreationNotebook = suggestedNotebookForCreation(
			$viewMode,
			$activeNotebook?.relative_path
		);
		noteCreationError = '';
		noteCreationOpen = true;
		void tick().then(() => creationDialog?.focus());
	}

	function createAndFocusNote() {
		requestNoteCreation();
	}

	function requestWebClip() {
		if ($shutdownPending || $viewMode === 'quickaccess' || $viewMode === 'trash' || $viewMode === 'unfiled') return;
		if (!isCompact && $notelistCollapsed) $notelistCollapsed = false;
		suggestedWebClipNotebook = suggestedNotebookForCreation(
			$viewMode,
			$activeNotebook?.relative_path
		);
		webClipUrl = '';
		webClipError = '';
		webClipOpen = true;
		void tick().then(() => webClipUrlInput?.focus() ?? webClipDialog?.focus());
	}

	async function confirmWebClipCategory(category: ParaCategory) {
		if ($shutdownPending || webClipBusy || !canSubmitWebClip(webClipUrl)) return;
		webClipBusy = true;
		webClipError = '';
		try {
			if (!(await ensureCurrentNoteSaved('Web clipping'))) return;
			const destination = destinationForCategory(category, suggestedWebClipNotebook);
			const entry = await clipWebPage(destination, cleanClipUrlInput(webClipUrl));
			await Promise.all([sidebar?.refresh(), noteList?.refresh(true)]);
			const content = await readNote(entry.path);
			if ($shutdownPending || !(await ensureCurrentNoteSaved('Opening the clipped note')) || $shutdownPending) return;
			if (!commitNote(entry.path, content)) return;
			webClipOpen = false;
			if (isCompact) $compactView = 'editor';
		} catch (error) {
			webClipError = webClipFailureMessage(error);
		} finally {
			webClipBusy = false;
		}
	}

	async function confirmNoteCategory(category: ParaCategory) {
		if ($shutdownPending || noteCreationBusy) return;
		noteCreationBusy = true;
		noteCreationError = '';
		try {
			const destination = destinationForCategory(category, suggestedCreationNotebook);
			if (noteCreationSource === 'wiki-link') {
				await editor?.createLinkedNoteAfterConfirmation(destination, noteCreationTitle);
			} else {
				await noteList?.createNoteAfterConfirmation(destination);
			}
			noteCreationOpen = false;
			await tick();
			editor?.focusTitle();
			if (isCompact) $compactView = 'editor';
		} catch {
			noteCreationError = 'Could not create the note. Choose a category to retry, or cancel.';
		} finally {
			noteCreationBusy = false;
		}
	}

	async function mutateTask(
		task: TaskItem,
		mutation: () => Promise<NoteContent>,
		errorMessage: string,
	): Promise<void> {
		if ($shutdownPending) return;
		const run = navigationQueue.then(async () => {
			if ($shutdownPending) return;
			const sourcePath = task.note_path;
			const wasActive = sourcePath === $activeNotePath;
			try {
				if (!wasActive) {
					await mutation();
					return;
				}
				if (!editor) return;
				await runActiveDocumentMutation({
					expectedPath: sourcePath,
					currentPath: () => $activeNotePath,
					isBlocked: () => $shutdownPending,
					prepare: () => editor!.lockMutations(),
					flush: async () => {
						const result = await editor!.flushSave();
						await reportSaveResult('Updating the task', result);
						return result;
					},
					mutate: mutation,
					commit: (content) => {
						$activeNote = content;
						editor?.loadNote(sourcePath, content.content, undefined, false, content.revision);
					},
				});
			} catch (error) {
				// A rejected task edit — an impossible due date, a note that moved — left the
				// Markdown unchanged. Saying so is the only way the user learns the click did
				// nothing; a packaged build has no console to read.
				console.error(errorMessage, error);
				showToast(describeLoadFailure(error));
			}
		});
		navigationQueue = run.then(() => {}, () => {});
		await run;
	}

	async function toggleTask(task: TaskItem) {
		await mutateTask(
			task,
			() => setTaskDone(task.note_path, task.line, task.raw_line, !task.completed),
			'Failed to toggle task:',
		);
	}

	async function changeTaskPriority(task: TaskItem, priority: string | null) {
		await mutateTask(
			task,
			() => setTaskPriority(task.note_path, task.line, task.raw_line, priority),
			'Failed to set task priority:',
		);
	}

	async function changeTaskDue(task: TaskItem, due: string | null) {
		await mutateTask(
			task,
			() => setTaskDue(task.note_path, task.line, task.raw_line, due),
			'Failed to set task due date:',
		);
	}

	/**
	 * Reload the notes that have no category.
	 *
	 * Called after opening a vault, because reconciliation on open may have set notes
	 * aside, and after filing one, so the count reflects what is left.
	 */
	async function refreshUnfiled(isCurrent: () => boolean = () => true) {
		try {
			const nextUnfiledNotes = await listUnfiledNotes();
			if (isCurrent()) $unfiledNotes = nextUnfiledNotes;
		} catch (e) {
			if (isCurrent()) console.error('Failed to load unfiled notes:', e);
		}
	}

	function compactBack() {
		if ($compactView === 'editor') $compactView = 'notelist';
		else if ($compactView === 'notelist') $compactView = 'sidebar';
	}

	function toggleSourceMode() {
		if (!$holdingPreview) $sourceMode = !$sourceMode;
	}

	// A note just moved to trash leaves the list, the manual order, and the editor.
	function forgetTrashedNote(path: string) {
		if (Object.hasOwn($noteOrder, path)) {
			const { [path]: _, ...rest } = $noteOrder;
			$noteOrder = rest;
		}
		$notes = $notes.filter((note) => note.path !== path);
		if ($activeNotePath === path) {
			$activeNote = null;
			$activeNotePath = null;
			if (isCompact) $compactView = 'notelist';
		}
		noteList?.refresh(true).catch((error) => console.error('Failed to refresh notes after trashing:', error));
	}

	async function trashOpenNote(path: string): Promise<boolean> {
		if ($shutdownPending || path !== $activeNotePath || $viewerNote || $viewMode === 'trash') return false;
		try {
			await deleteNote(path);
			forgetTrashedNote(path);
			return true;
		} catch (error) {
			console.error('Failed to move open note to Trash:', error);
			return false;
		}
	}

	// A similar-note append rewrites one note and trashes another; the editor must not keep
	// unsaved edits to either, nor show the old text afterwards.
	async function beforeSimilarAppend(paths: string[]): Promise<boolean> {
		if (!$activeNotePath || !paths.includes($activeNotePath)) return true;
		return ensureCurrentNoteSaved('Adding to a similar note');
	}

	function afterSimilarAppend(capturePath: string, targetPath: string, captureTrashed: boolean) {
		if ($activeNotePath === targetPath) void reloadOpenNoteFromDisk();
		if (captureTrashed) forgetTrashedNote(capturePath);
		else noteList?.refresh(true).catch((error) => console.error('Failed to refresh notes after appending:', error));
	}

	function handleMouseDown(e: MouseEvent) {
		if (e.button === 3) { e.preventDefault(); navigateHistory(-1); }
		if (e.button === 4) { e.preventDefault(); navigateHistory(1); }
	}

	function handleKeydown(e: KeyboardEvent) {
		if ($shutdownPending) {
			e.preventDefault();
			return;
		}
		if (noteCreationOpen) {
			if (e.code === 'Escape' && !noteCreationBusy) {
				e.preventDefault();
				noteCreationOpen = false;
			}
			return;
		}
		if (webClipOpen) {
			if (e.code === 'Escape' && !webClipBusy) {
				e.preventDefault();
				webClipOpen = false;
			}
			return;
		}
		const mod = e.ctrlKey || e.metaKey;
		const code = e.code;

		// Mod+K (insert link) is an editor formatting action and is not user-customizable.
		if (mod && code === 'KeyK' && !e.shiftKey && $activeNotePath && !$sourceMode) {
			e.preventDefault();
			editor?.addLinkFromToolbar();
		}

		// Ctrl/Cmd+Shift+Delete: move the open note to the trash.
		if (mod && e.shiftKey && code === 'Delete' && $activeNotePath && !$holdingPreview) {
			e.preventDefault();
			editor?.moveOpenNoteToTrash();
			return;
		}

		const action = matchAction(e, $keybindings);
		if (action) {
			e.preventDefault();
			switch (action) {
				case 'nav-back':
					navigateHistory(-1);
					return;
				case 'nav-forward':
					navigateHistory(1);
					return;
				case 'new-note':
					createAndFocusNote();
					return;
				case 'toggle-theme': {
					const isDark = isDarkTheme($resolvedTheme, $customThemes);
					const next = isDark ? 'light' : 'dark';
					$theme = next;
					setTheme(next);
					return;
				}
				case 'search-vault':
					$showSearch = true;
					return;
				case 'find-in-note':
					if ($activeNotePath) {
						editor?.openNoteSearch();
					} else {
						$showSearch = true;
					}
					return;
				case 'quick-open':
					$showCommandPalette = true;
					return;
				case 'save':
					editor?.forceSave();
					return;
				case 'toggle-source':
					toggleSourceMode();
					return;
				case 'open-new-window':
					if ($activeNotePath && $activeNote && !$holdingPreview) {
						void openNoteInSecondaryWindow($activeNotePath, $activeNote.meta.title);
					}
					return;
				case 'toggle-sidebar':
					$sidebarCollapsed = !$sidebarCollapsed;
					return;
				case 'toggle-notelist':
					toggleNoteList();
					return;
				case 'toggle-focus':
					if ($holdingPreview) return;
					$focusMode = !$focusMode;
					return;
				case 'toggle-readonly':
					if ($viewerNote || $holdingPreview) return; // preview modes are always read-only
					$readOnly = !$readOnly;
					return;
				case 'fullscreen':
					appWindow.isFullscreen().then(fs => appWindow.setFullscreen(!fs));
					return;
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

	$effect(() => {
		$collapsedNotebooks;
		persistState();
	});

	$effect(() => {
		$activeNotePath;
		persistState();
	});

	$effect(() => {
		$sidebarCollapsed;
		$notelistCollapsed;
		$outlineWidth;
		persistState();
	});

	$effect(() => {
		$notebookSortMode;
		$notebookOrder;
		persistState();
	});

	$effect(() => {
		$noteOrder;
		persistState();
	});

	$effect(() => {
		$sortMode;
		$groupNotesByDate;
		$tasksLayout;
		$tasksHideCompleted;
		$tasksOnlyFlagged;
		$tasksSort;
		persistState();
	});

	$effect(() => {
		$viewMode;
		$activeNotebook;
		$activeTag;
		persistState();
	});

	onMount(async () => {
		const lifetime = lifetimeGate.capture();
		const restoration = startupGate.capture();
		const alive = () => lifetimeGate.isCurrent(lifetime);
		const handleNavigationRequest = (event: Event) => {
			const path = (event as CustomEvent<NavigateNoteRequest>).detail?.path;
			if (path) void navigateToPath(path);
		};
		window.addEventListener(NAVIGATE_NOTE_EVENT, handleNavigationRequest);
		removeNavigationRequest = () => window.removeEventListener(NAVIGATE_NOTE_EVENT, handleNavigationRequest);

		unlistenSyncDone = await listenAppEvent('syncDone', async (event) => {
			if (alive() && event.payload.outcome !== 'failure') await refreshAfterSync();
		});
		if (!alive()) { unlistenSyncDone(); unlistenSyncDone = null; return; }

		let lastNotePath: string | null = null;
		let lastViewMode = '';
		let lastNotebook: string | null = null;
		let lastTag: string | null = null;
		try {
			const state = await loadVaultState();
			if (!alive()) return;
			$sidebarWidth = state.sidebar_width;
			$notelistWidth = state.notelist_width;
			if (typeof state.outline_width === 'number') $outlineWidth = state.outline_width;
			$sidebarCollapsed = state.sidebar_collapsed;
			$notelistCollapsed = state.notelist_collapsed ?? false;
			$collapsedNotebooks = state.collapsed_notebooks ?? [];
			$notebookSortMode = state.notebook_sort_mode === 'manual' ? 'manual' : 'alphabetical';
			$notebookOrder = state.notebook_order ?? {};
			$noteOrder = state.note_order ?? {};
			if (state.sort_mode === 'created' || state.sort_mode === 'title' || state.sort_mode === 'modified' || state.sort_mode === 'custom') $sortMode = state.sort_mode;
			if (typeof state.group_notes_by_date === 'boolean') $groupNotesByDate = state.group_notes_by_date;
			if (state.tasks_layout === 'calendar' || state.tasks_layout === 'list') $tasksLayout = state.tasks_layout;
			if (typeof state.tasks_hide_completed === 'boolean') $tasksHideCompleted = state.tasks_hide_completed;
			if (typeof state.tasks_only_flagged === 'boolean') $tasksOnlyFlagged = state.tasks_only_flagged;
			if (state.tasks_sort === 'due' || state.tasks_sort === 'priority' || state.tasks_sort === 'note') $tasksSort = state.tasks_sort;
			lastNotePath = state.last_open_note ?? null;
			lastViewMode = state.last_view_mode ?? '';
			lastNotebook = state.last_notebook ?? null;
			lastTag = state.last_tag ?? null;
		} catch (_) {}
		if (!alive()) return;

		// Opening the vault reconciles note locations against their categories, which may
		// have set notes aside for want of one. Load them so the user is told.
		await refreshUnfiled(alive);
		if (!alive()) return;
		try {
			repairStatus = await getRepairStatus();
		} catch (error) {
			repairError = String(error);
		}
		if (!alive()) return;
		const repairUnlisten = await listenAppEvent('repairStatusChanged', (event) => {
			if (!alive()) return;
			repairStatus = event.payload;
			repairError = '';
		});
		if (!alive()) { repairUnlisten(); return; }
		unlistenRepairStatus = repairUnlisten;

		const restoreLastSession = $appConfig?.restore_last_session === true;

		// The sidebar supplies notebook identities for the saved view. Select that view
		// before loading notes, so startup reads the chosen list only once.
		await sidebar?.refresh({ deferTags: true });
		if (!alive()) return;

		orphanScanTimer = setTimeout(async () => {
			if (!alive() || get(editorDirty)) return; // skip if user is actively editing
			try {
				const orphans = await findOrphanedAttachments();
				if (!alive()) return;
				if (orphans.length > 0) {
					if (get(editorDirty)) return; // re-check after async scan
					const moved = await trashOrphanedAttachments(orphans.map((o) => o.name));
					if (moved > 0) {
						const toast = document.createElement('div');
						toast.style.cssText = 'position:fixed;bottom:24px;left:50%;transform:translateX(-50%);background:var(--bg-secondary);color:var(--text-primary);padding:10px 18px;border-radius:8px;box-shadow:0 4px 12px rgba(0,0,0,.15);border:1px solid var(--border-color);font-size:13px;z-index:10000;opacity:0;transition:opacity .3s;pointer-events:none';
						toast.textContent = `Moved ${moved} orphaned attachment${moved > 1 ? 's' : ''} to trash`;
						document.body.appendChild(toast);
						requestAnimationFrame(() => (toast.style.opacity = '1'));
						setTimeout(() => { toast.style.opacity = '0'; setTimeout(() => toast.remove(), 300); }, 4000);
					}
				}
			} catch (_) {}
		}, 3000);

		const startupTarget = resolveStartupTarget({
			startupView: $appConfig?.startup_view,
			restoreLastSession,
			lastViewMode,
			lastNotebook,
			lastTag
		});
		if (startupGate.isCurrent(restoration) && alive()) {
			if (!applyStartupTarget(startupTarget) && startupGate.isCurrent(restoration) && alive()) {
				applyStartupTarget({ mode: normalizeStartupView($appConfig?.startup_view) });
			}
		}
		if (!alive()) return;
		await noteList?.refresh();
		if (!alive()) return;

		// Reopen the last note only when session restoration is enabled and no interaction
		// has advanced the startup generation. Commit through the navigation queue.
		if (restoreLastSession && lastNotePath && startupGate.isCurrent(restoration)) {
			const restoreRun = navigationQueue.then(async () => {
				if (!alive() || !startupGate.isCurrent(restoration) || $activeNotePath || $editorDirty || $shutdownPending) return;
				try {
					const content = await readNote(lastNotePath!);
					if (!alive() || !startupGate.isCurrent(restoration) || $activeNotePath || $editorDirty || $shutdownPending) return;
					$activeNote = content;
					$activeNotePath = lastNotePath;
					editor?.loadNote(lastNotePath!, content.content, undefined, false, content.revision);
				} catch (_) {}
			});
			navigationQueue = restoreRun.then(() => {}, () => {});
			await restoreRun;
		}
		if (!alive()) return;
		restorationSettled = true;
		persistState();
		void markStartupReady();
		void sidebar?.refreshTags();
		void installEditorKeyProbe();

		const debouncedVaultRefresh = debounce(async () => {
			if (!alive()) return;
			void reloadOpenNoteFromDisk();
			await Promise.all([sidebar?.refresh(), noteList?.refresh(true)]);
		}, 300);
		unlistenFileChange = await listenAppEvent('fileChanged', () => {
			if (alive()) debouncedVaultRefresh();
		});
		if (!alive()) { unlistenFileChange?.(); return; }


		// The backend polls for reachability and announces changes, so AI features come
		// back on their own when the other machine wakes, with no restart.
		unlistenAiStatus = await listenAppEvent('aiStatusChanged', (event) => {
			if (alive()) $aiStatus = event.payload;
		});
		if (!alive()) { unlistenAiStatus(); unlistenAiStatus = null; return; }
		try {
			const status = await getAiStatus();
			if (!alive()) return;
			$aiStatus = status;
		} catch (e) {
			console.error('Failed to read AI status:', e);
		}

		// Registration happens once at startup and can take a moment (the very first launch
		// may prompt), so the panel needs to hear about it rather than poll for it.
		unlistenHotkeyStatus = await listenAppEvent('hotkeyStatusChanged', (event) => {
			if (alive()) $hotkeyStatus = event.payload;
		});
		if (!alive()) { unlistenHotkeyStatus(); unlistenHotkeyStatus = null; return; }
		try {
			const status = await getHotkeyStatus();
			if (!alive()) return;
			$hotkeyStatus = status;
		} catch (e) {
			console.error('Failed to read hotkey status:', e);
		}

		unlistenOpenFile = await listenAppEvent('openFile', async (event) => {
			if (alive()) await handleOpenFile(event.payload);
		});
		if (!alive()) { unlistenOpenFile(); unlistenOpenFile = null; return; }

		// Check for pending file from first-launch CLI args
		try {
			const pending = await getPendingOpenFile();
			if (!alive()) return;
			if (pending) await handleOpenFile(pending);
			if (!alive()) return;
		} catch (_) {}

		// Scheduled backup: check on startup and every 5 minutes
		checkScheduledBackup();
		backupInterval = setInterval(checkScheduledBackup, 5 * 60 * 1000);

		// Notion: once at startup, then on its own interval. The tick always runs, even when
		// Notion is not set up yet — gating it on startup state would mean connecting in
		// Settings mid-session publishes nothing automatically until the next launch. The
		// interval is read once rather than every tick, which would walk the map to learn one
		// number.
		try {
			const status = await notionStatus();
			if (!alive()) return;
			notionPollMs = Math.max(1, status.poll_minutes) * 60 * 1000;
		} catch (_) {}
		if (!alive()) return;
		checkScheduledNotion();
		notionInterval = setInterval(checkScheduledNotion, 60 * 1000);

	});

	onDestroy(() => {
		lifetimeGate.cancel();
		if (openNoteReloadRetry) clearTimeout(openNoteReloadRetry);
		startupGate.cancel();
		void releaseOwnedVaultSwitchGate();
		unlistenFileChange?.();
		unlistenSyncDone?.();
		unlistenAiStatus?.();
		unlistenHotkeyStatus?.();
		unlistenRepairStatus?.();
		unlistenOpenFile?.();
		removeNavigationRequest?.();
		if (backupInterval) clearInterval(backupInterval);
		if (notionInterval) clearInterval(notionInterval);
		if (orphanScanTimer) clearTimeout(orphanScanTimer);
	});
</script>

<svelte:window onkeydown={handleKeydown} onmousedown={isCompact ? undefined : handleMouseDown} />

{#if $shutdownPending}
	<div class="shutdown-lock" aria-label="Saving before close" aria-busy="true"></div>
{/if}

{#if noteCreationOpen}
	<div class="creation-backdrop">
		<button
			class="creation-dismiss"
			type="button"
			aria-label="Cancel new note"
			disabled={noteCreationBusy}
			onclick={() => noteCreationOpen = false}
		></button>
		<div bind:this={creationDialog} class="creation-dialog" role="dialog" aria-modal="true" aria-labelledby="creation-title" tabindex="-1">
			<h2 id="creation-title">Where should this note go?</h2>
			<p>Choose a category before “{noteCreationTitle}” is created.</p>
			{#if noteCreationError}<p class="creation-error" role="alert">{noteCreationError}</p>{/if}
			<div class="creation-categories">
				{#each PARA_CATEGORIES as category}
					<button type="button" onclick={() => confirmNoteCategory(category)} disabled={noteCreationBusy}>
						<strong>{category}</strong>
						{#if suggestedCreationNotebook?.startsWith(`${category}/`)}
							<span>{suggestedCreationNotebook.slice(category.length + 1)}</span>
						{/if}
					</button>
				{/each}
			</div>
			<button class="creation-cancel" type="button" onclick={() => noteCreationOpen = false} disabled={noteCreationBusy}>Cancel</button>
		</div>
	</div>
{/if}

{#if webClipOpen}
	<div class="creation-backdrop">
		<button
			class="creation-dismiss"
			type="button"
			aria-label="Cancel web clipping"
			disabled={webClipBusy}
			onclick={() => webClipOpen = false}
		></button>
		<div bind:this={webClipDialog} class="creation-dialog web-clip-dialog" role="dialog" aria-modal="true" aria-labelledby="web-clip-title" tabindex="-1">
			<h2 id="web-clip-title">Clip web page</h2>
			<label class="web-clip-field">
				<span>URL</span>
				<input
					bind:this={webClipUrlInput}
					bind:value={webClipUrl}
					type="url"
					inputmode="url"
					placeholder="https://example.com/article"
					disabled={webClipBusy}
				>
			</label>
			<p>Choose where the clipping should be filed.</p>
			{#if webClipError}<p class="creation-error" role="alert">{webClipError}</p>{/if}
			<div class="creation-categories">
				{#each PARA_CATEGORIES as category}
					<button type="button" onclick={() => confirmWebClipCategory(category)} disabled={webClipBusy || !canSubmitWebClip(webClipUrl)}>
						<strong>{category}</strong>
						{#if suggestedWebClipNotebook?.startsWith(`${category}/`)}
							<span>{suggestedWebClipNotebook.slice(category.length + 1)}</span>
						{/if}
					</button>
				{/each}
			</div>
			<button class="creation-cancel" type="button" onclick={() => webClipOpen = false} disabled={webClipBusy}>Cancel</button>
		</div>
	</div>
{/if}

{#if banner}
	<div class="repair-banner" role={banner.kind === 'repair' ? 'alert' : 'status'}>
		<div>
			<strong>{banner.title}</strong>
			<span>{banner.message}</span>
			{#each banner.paths as path (path)}
				<code>{path}</code>
			{/each}
		</div>
		{#if banner.kind === 'repair'}
			<button type="button" onclick={repairNow} disabled={repairBusy}>
				{repairBusy ? 'Repairing…' : 'Repair now'}
			</button>
		{:else}
			<button type="button" onclick={() => banner.key && dismissNotice(banner.key)} disabled={repairBusy}>Dismiss</button>
		{/if}
	</div>
{/if}

<!-- Rendered by whichever layout is active. -->
{#snippet sidebarView()}
	<Sidebar bind:this={sidebar} onViewChanged={handleViewChanged} onRelocateActiveDocument={relocateActiveDocument} />
{/snippet}
{#snippet noteListView()}
	<NoteList bind:this={noteList} onOpenNote={navigateToPath} onBeforeNoteSwitch={() => ensureCurrentNoteSaved('Navigation')} onBeforeNoteDuplicate={() => ensureCurrentNoteSaved('Duplicating the note')} onBeforeOpenWindow={() => ensureCurrentNoteSaved('Opening a secondary window')} onRelocateActiveDocument={relocateActiveDocument} onUpdateActiveMetadata={updateActiveMetadata} onNoteMoved={() => sidebar?.refresh()} onNoteCreated={() => { editor?.focusTitle(); }} onRequestCreateNote={requestNoteCreation} onToggleTask={toggleTask} onSetTaskPriority={changeTaskPriority} onSetTaskDue={changeTaskDue} />
{/snippet}

<!-- One shell for both layouts, so Editor stays mounted when the window crosses the compact
     breakpoint: remounting it would drop unsaved edits and reload stale content. -->
<div class={isCompact ? 'compact-shell' : 'app-shell'}>
	{#if isCompact}
		<div class="compact-header">
			{#if $compactView !== 'sidebar'}
				<button class="compact-header-btn" onclick={compactBack} aria-label="Go back">
					<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<path d="M15 18l-6-6 6-6"/>
					</svg>
				</button>
			{:else}
				<div class="compact-header-brand">
					<svg width="22" height="22" viewBox="0 0 48 48" fill="none">
						<rect width="48" height="48" rx="12" fill="var(--accent)" />
						<circle cx="16" cy="16" r="3.5" fill="white" opacity="0.9" />
						<circle cx="32" cy="16" r="3.5" fill="white" opacity="0.9" />
						<circle cx="16" cy="32" r="3.5" fill="white" opacity="0.9" />
						<circle cx="32" cy="32" r="3.5" fill="white" opacity="0.9" />
					</svg>
				</div>
			{/if}
			{#if $compactView !== 'editor'}
				<span class="compact-header-title">
					{#if $compactView === 'sidebar'}
						Second Brain
					{:else}
						{#if $viewMode === 'notebook'}{$activeNotebook?.name ?? 'Notebook'}{:else if $viewMode === 'tag'}#{$activeTag}{:else if $viewMode === 'quickaccess'}Quick Access{:else if $viewMode === 'tasks'}Tasks{:else if $viewMode === 'unfiled'}Unfiled{:else if $viewMode === 'trash'}Trash{:else}All Notes{/if}
					{/if}
				</span>
			{/if}
			<div class="compact-header-actions">
				{#if $compactView === 'editor' && !$holdingPreview && !$viewerNote}
					<button class="compact-header-btn" class:active={$readOnly} onclick={() => { if (!$shutdownPending) $readOnly = !$readOnly; }} disabled={$shutdownPending} title={$readOnly ? 'Edit' : 'View'}>
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
					<button class="compact-header-btn" onclick={() => editor?.openNoteSearch()} title="Find in note">
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>
						</svg>
					</button>
					<button class="compact-header-btn" class:active={$activeNote?.meta.pinned} onclick={() => editor?.togglePinned()} disabled={$shutdownPending} title="Pin">
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<path d="M12 17v5"/><path d="M9 2h6l-1 7h4l-2 4H8l-2-4h4L9 2z"/>
						</svg>
					</button>
					<button class="compact-header-btn" class:active={isQuickAccess} onclick={async () => {
						if (!noteRelativePath) return;
						try {
							if (isQuickAccess) { await removeQuickAccess(noteRelativePath); } else { await addQuickAccess(noteRelativePath); }
							$quickAccessPaths = (await getQuickAccess()).map(n => n.relative_path);
						} catch (e) { console.error('Quick access toggle failed:', e); }
					}} title="Quick Access">
						<svg width="18" height="18" viewBox="0 0 24 24" fill={isQuickAccess ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
						</svg>
					</button>
					<button class="compact-header-btn" onclick={() => editor?.toggleOutlinePanel()} title="Outline">
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<line x1="4" y1="6" x2="20" y2="6"/><line x1="8" y1="12" x2="20" y2="12"/><line x1="8" y1="18" x2="20" y2="18"/><circle cx="4" cy="12" r="1" fill="currentColor"/><circle cx="4" cy="18" r="1" fill="currentColor"/>
						</svg>
					</button>
					<button class="compact-header-btn" onclick={() => editor?.toggleHistoryPanel()} title="History">
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>
						</svg>
					</button>
					{#if $appConfig?.enable_wiki_links}
					<button class="compact-header-btn" onclick={() => editor?.toggleGraphView()} title="Graph View">
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<circle cx="6" cy="6" r="3"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="18" r="3"/>
							<line x1="8.5" y1="7.5" x2="15.5" y2="16.5"/><line x1="15.5" y1="7.5" x2="8.5" y2="16.5"/>
						</svg>
					</button>
					{/if}
					{#if $appConfig?.ai_provider}
					<button class="compact-header-btn" onclick={() => editor?.triggerAiMenu()} title="AI Actions">
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<path d="M12 8V4l-2-2"/><rect x="4" y="8" width="16" height="12" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M9 13v2"/><path d="M15 13v2"/>
						</svg>
					</button>
					{/if}
					<button class="compact-header-btn" class:active={$sourceMode} onclick={toggleSourceMode} title={$sourceMode ? 'Rich Editor' : 'Source Mode'}>
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<polyline points="16 18 22 12 16 6" /><polyline points="8 6 2 12 8 18" />
						</svg>
					</button>
					{:else}
						<button class="compact-header-btn" onclick={requestWebClip} title="Clip web page">
							<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M10 13a5 5 0 007.54.54l3-3a5 5 0 00-7.07-7.07l-1.72 1.71" />
								<path d="M14 11a5 5 0 00-7.54-.54l-3 3a5 5 0 007.07 7.07l1.71-1.71" />
							</svg>
						</button>
						<button class="compact-header-btn" onclick={() => ($showSearch = true)} title="Search">
							<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<circle cx="11" cy="11" r="8" />
							<line x1="21" y1="21" x2="16.65" y2="16.65" />
						</svg>
					</button>
					<button class="compact-header-btn" onclick={() => ($showInfo = true)} title="Info">
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<circle cx="12" cy="12" r="10" />
							<line x1="12" y1="16" x2="12" y2="12" />
							<line x1="12" y1="8" x2="12.01" y2="8" />
						</svg>
					</button>
					<button class="compact-header-btn" onclick={() => ($showSettings = true)} title="Settings">
						<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<circle cx="12" cy="12" r="3" />
							<path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 01-2.83 2.83l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
						</svg>
					</button>
				{/if}
			</div>
		</div>
	{:else if $focusMode}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="focus-topbar" onmousedown={(e) => { if (!(e.target as HTMLElement).closest('button')) appWindow.startDragging(); }}>
			<span class="focus-title">{$activeNote?.meta.title || 'Untitled'}</span>
			<div class="focus-controls">
				<button class="focus-btn focus-active" onclick={() => ($focusMode = false)} title="Exit focus mode (Escape)">
					<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<path d="M8 3v3a2 2 0 01-2 2H3m18 0h-3a2 2 0 01-2-2V3m0 18v-3a2 2 0 012-2h3M3 16h3a2 2 0 012 2v3"/>
					</svg>
				</button>
				{#if !$holdingPreview && !$viewerNote}<button class="focus-btn" class:focus-active={$readOnly} onclick={() => ($readOnly = !$readOnly)} title={$readOnly ? 'Switch to Edit Mode' : 'Switch to View Mode'}>
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
				</button>{/if}
									<button class="focus-btn" onmousedown={(e) => e.stopPropagation()} onclick={() => appWindow.minimize()} title="Minimize">
					<svg width="10" height="10" viewBox="0 0 10 10"><line x1="1" y1="5" x2="9" y2="5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
				</button>
				<button class="focus-btn" onmousedown={(e) => e.stopPropagation()} onclick={() => appWindow.toggleMaximize()} title="Maximize">
					<svg width="10" height="10" viewBox="0 0 10 10"><rect x="1" y="1" width="8" height="8" rx="1" fill="none" stroke="currentColor" stroke-width="1.2"/></svg>
				</button>
				<button class="focus-btn focus-close" onmousedown={(e) => e.stopPropagation()} onclick={() => appWindow.close()} title="Close">
					<svg width="10" height="10" viewBox="0 0 10 10"><line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
				</button>
			</div>
		</div>
	{:else}
		<TitleBar onNewNote={createAndFocusNote} onClipWeb={requestWebClip} onSelectNote={selectNoteFromSwitcher} onOpenWindow={openNoteInSecondaryWindow} onRequestVaultSwitch={requestVaultSwitch} />
	{/if}
	<div class={isCompact ? 'compact-content' : 'app-layout'}>
		{#if isCompact}
			<div class="compact-panel" class:active={$compactView === 'sidebar'}>
				{@render sidebarView()}
			</div>
			<div class="compact-panel" class:active={$compactView === 'notelist'}>
				{@render noteListView()}
			</div>
		{:else if !$focusMode}
			<div class="sidebar-panel" style="width: {$sidebarCollapsed ? 44 : $sidebarWidth}px">
				{@render sidebarView()}
			</div>

			{#if !$sidebarCollapsed}
				<ResizeHandle onResize={handleSidebarResize} />
			{/if}

			{#if !$notelistCollapsed}
				<div class="notelist-panel" style="width: {$notelistWidth}px">
					{@render noteListView()}
				</div>

				<ResizeHandle onResize={handleNotelistResize} />
			{:else}
				<div class="notelist-restore-panel">
					<button class="notelist-restore-btn" onclick={revealNoteList} title={`Show notes list (Ctrl+Shift+\\)`} aria-label="Show notes list">
						<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<line x1="8" y1="6" x2="20" y2="6" />
							<line x1="8" y1="12" x2="20" y2="12" />
							<line x1="8" y1="18" x2="20" y2="18" />
							<polyline points="5 8 3 12 5 16" />
						</svg>
					</button>
				</div>
			{/if}
		{/if}
		<div class={isCompact ? 'compact-panel' : 'editor-panel'} class:active={isCompact && $compactView === 'editor'}>
			<Editor bind:this={editor} onMoveToTrash={trashOpenNote} onRequestCreateLinkedNote={requestLinkedNoteCreation} onNavigateNote={navigateToPath} onNavigateWikiNote={navigateToPathResult} onNavigateHistory={navigateHistory} onRelocateActiveDocument={relocateActiveDocument} />
			{#if !isCompact && $viewMode === 'tasks' && !taskNoteOpened}
				<div class="tasks-editor-placeholder">
					<svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
						<path d="M9 11l3 3L22 4"/><path d="M21 12v7a2 2 0 01-2 2H5a2 2 0 01-2-2V5a2 2 0 012-2h11"/>
					</svg>
					<p>Select a task to open its note</p>
				</div>
			{/if}
		</div>
	</div>
</div>

<SearchPanel onOpenResult={(path, noteId) => navigateToPathResult(path, undefined, false, noteId)} />
<SimilarCards onOpen={(path) => navigateToPath(path)} onBeforeAppend={beforeSimilarAppend} onAppended={afterSimilarAppend} />
<CommandPalette onNavigate={handleViewChanged} onToggleSource={toggleSourceMode} />
<SettingsPanel onRequestVaultSwitch={requestVaultSwitch} onBeforeRestore={prepareForRestore} onAfterRestore={refreshAfterRestore} onAfterConflictChoice={reloadOpenNoteFromDisk} />
<InfoPanel />

<style>
	.creation-backdrop {
		position: fixed;
		inset: 0;
		z-index: 10030;
		display: grid;
		place-items: center;
		padding: 20px;
		background: rgba(0, 0, 0, 0.42);
	}

	.creation-dialog {
		position: relative;
		z-index: 1;
		width: min(420px, 100%);
		padding: 20px;
		border: 1px solid var(--border-color);
		border-radius: 12px;
		background: var(--bg-primary);
		box-shadow: 0 18px 60px rgba(0, 0, 0, 0.32);
		color: var(--text-primary);
	}

	.creation-dismiss {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		padding: 0;
		border: 0;
		background: transparent;
		cursor: default;
	}

	.creation-dialog h2 {
		margin: 0 0 6px;
		font-size: 18px;
	}

	.creation-dialog p {
		margin: 0 0 16px;
		color: var(--text-secondary);
		font-size: 13px;
	}

	.creation-dialog .creation-error {
		color: var(--danger-color, #c33);
	}

	.web-clip-field {
		display: grid;
		gap: 6px;
		margin: 14px 0 10px;
	}

	.web-clip-field span {
		color: var(--text-secondary);
		font-size: 12px;
		font-weight: 600;
	}

	.web-clip-field input {
		width: 100%;
		padding: 9px 10px;
		border: 1px solid var(--border-color);
		border-radius: 8px;
		background: var(--bg-secondary);
		color: var(--text-primary);
		font: inherit;
		font-size: 13px;
		outline: none;
	}

	.web-clip-field input:focus {
		border-color: var(--accent-color);
		box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent-color) 18%, transparent);
	}

	.creation-categories {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 8px;
	}

	.creation-categories button {
		display: flex;
		min-height: 58px;
		flex-direction: column;
		align-items: flex-start;
		justify-content: center;
		padding: 10px 12px;
		border: 1px solid var(--border-color);
		border-radius: 8px;
		background: var(--bg-secondary);
		color: var(--text-primary);
		cursor: pointer;
	}

	.creation-categories button:hover:not(:disabled) {
		border-color: var(--accent-color);
		background: var(--bg-hover);
	}

	.creation-categories span {
		overflow: hidden;
		max-width: 100%;
		color: var(--text-secondary);
		font-size: 11px;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.creation-cancel {
		margin-top: 14px;
		padding: 6px 10px;
		border: 0;
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
	}

	.repair-banner {
		position: fixed;
		top: 38px;
		left: 50%;
		z-index: 10020;
		display: flex;
		align-items: center;
		gap: 14px;
		max-width: min(720px, calc(100vw - 24px));
		padding: 10px 12px;
		transform: translateX(-50%);
		border: 1px solid color-mix(in srgb, #d97706 55%, var(--border-color));
		border-radius: 8px;
		background: var(--bg-secondary);
		box-shadow: 0 6px 24px rgba(0, 0, 0, 0.22);
		color: var(--text-primary);
		font-size: 12px;
	}

	.repair-banner > div {
		display: flex;
		min-width: 0;
		flex: 1;
		flex-wrap: wrap;
		gap: 4px 8px;
	}

	.repair-banner span,
	.repair-banner code {
		color: var(--text-secondary);
	}

	.repair-banner code {
		overflow: hidden;
		max-width: 100%;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.repair-banner button {
		flex-shrink: 0;
		padding: 6px 10px;
		border: 1px solid var(--border-color);
		border-radius: 6px;
		background: var(--bg-tertiary);
		color: var(--text-primary);
		cursor: pointer;
	}

	.repair-banner button:disabled {
		opacity: 0.6;
		cursor: default;
	}

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

	.notelist-restore-panel {
		flex-shrink: 0;
		width: 36px;
		height: 100%;
		padding-top: 8px;
		display: flex;
		justify-content: center;
		background: var(--bg-primary);
		border-right: 1px solid var(--border-color);
	}

	.notelist-restore-btn {
		width: 24px;
		height: 24px;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 0;
		border: none;
		border-radius: 4px;
		background: transparent;
		color: var(--text-tertiary);
		cursor: pointer;
	}

	.notelist-restore-btn:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.editor-panel {
		flex: 1;
		height: 100%;
		overflow: hidden;
		min-width: 300px;
		position: relative;
	}

	.tasks-editor-placeholder {
		position: absolute;
		inset: 0;
		z-index: 5;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 12px;
		background: var(--bg-primary);
		color: var(--text-tertiary);
		font-size: 14px;
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


	/* ═══ COMPACT LAYOUT STYLES ═══ */
	.compact-shell {
		display: flex;
		flex-direction: column;
		height: 100dvh;
		box-sizing: border-box;
		padding:
			env(safe-area-inset-top, 0px)
			env(safe-area-inset-right, 0px)
			env(safe-area-inset-bottom, 0px)
			env(safe-area-inset-left, 0px);
		overflow: hidden;
		background: var(--bg-primary);
	}

	.compact-header {
		display: flex;
		align-items: center;
		height: 52px;
		padding: 0 8px;
		background: var(--bg-secondary);
		border-bottom: 1px solid var(--border-color);
		flex-shrink: 0;
		gap: 4px;
	}

	.compact-header-brand {
		display: flex;
		align-items: center;
		padding: 0 8px;
	}

	.compact-header-title {
		flex: 1;
		font-size: 17px;
		font-weight: 600;
		color: var(--text-primary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		padding: 0 4px;
	}

	.compact-header-actions {
		display: flex;
		align-items: center;
		gap: 2px;
		margin-left: auto;
	}

	.compact-header-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 34px;
		height: 34px;
		border: none;
		background: none;
		color: var(--text-secondary);
		border-radius: 8px;
		cursor: pointer;
	}

	.compact-header-btn svg {
		width: 16px;
		height: 16px;
	}

	.compact-header-btn:active {
		background: var(--bg-hover);
	}

	.compact-header-btn.active {
		color: var(--accent);
	}

	.compact-content {
		flex: 1;
		overflow: hidden;
		position: relative;
	}

	.compact-panel {
		position: absolute;
		inset: 0;
		overflow: hidden;
		visibility: hidden;
		pointer-events: none;
		display: flex;
		flex-direction: column;
	}

	.compact-panel.active {
		visibility: visible;
		pointer-events: auto;
	}

	.shutdown-lock {
		position: fixed;
		inset: 0;
		z-index: 2147483647;
		cursor: wait;
	}

</style>
