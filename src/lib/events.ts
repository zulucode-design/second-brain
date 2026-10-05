import { listen, type Event, type UnlistenFn } from '@tauri-apps/api/event';
import type { AiStatus, AiStreamEvent, BackupEntry, BulkMutationTerminal, FileEvent, HotkeyStatus, RepairStatus, SimilarityCheck } from '$lib/types';
import type { ImportDonePayload } from '$lib/utils/import-outcome';
import type { NotionSummary } from '$lib/utils/notion-settings';

/** Wire names shared with src-tauri/src/events.rs. */
export const EVENTS = {
  aiStream: 'ai-stream',
  aiStatusChanged: 'ai-status-changed',
  aiTestResult: 'ai-test-result',
  backupDone: 'backup-done',
  editorFontSizeChanged: 'editor-font-size-changed',
  fileChanged: 'file-changed',
  hotkeyStatusChanged: 'hotkey-status-changed',
  importDone: 'import-done',
  notionPublishFailed: 'notion-publish-failed',
  notionPublishFinished: 'notion-publish-finished',
  notionPublishProgress: 'notion-publish-progress',
  openFile: 'open-file',
  quickCaptureShown: 'quick-capture-shown',
  repairStatusChanged: 'repair-status-changed',
  restoreDone: 'restore-done',
  saveBeforeClose: 'save-before-close',
  saveCloseReleased: 'save-close-released',
  similarNotesFound: 'similar-notes-found',
  syncDone: 'sync-done',
  uiScaleChanged: 'ui-scale-changed',
} as const;

export interface EventPayloads {
  aiStream: AiStreamEvent;
  aiStatusChanged: AiStatus;
  aiTestResult: { success: boolean; message?: string; error?: string };
  backupDone: { success: boolean; entry?: BackupEntry; error?: string };
  editorFontSizeChanged: number;
  fileChanged: FileEvent;
  hotkeyStatusChanged: HotkeyStatus;
  importDone: ImportDonePayload;
  notionPublishFailed: { error: string; fatal: boolean };
  notionPublishFinished: NotionSummary;
  notionPublishProgress: { done: number; total: number };
  openFile: string;
  quickCaptureShown: null;
  repairStatusChanged: RepairStatus;
  restoreDone: BulkMutationTerminal;
  saveBeforeClose: { requestId: string };
  saveCloseReleased: { requestId: string; timedOut: boolean };
  similarNotesFound: SimilarityCheck;
  syncDone: BulkMutationTerminal;
  uiScaleChanged: number;
}

export function listenAppEvent<K extends keyof EventPayloads>(
  name: K,
  handler: (event: Event<EventPayloads[K]>) => void,
): Promise<UnlistenFn> {
  return listen<EventPayloads[K]>(EVENTS[name], handler);
}
