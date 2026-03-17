import { writable, derived } from "svelte/store";
import type {
  AppConfig,
  NoteEntry,
  NoteContent,
  NotebookEntry,
  VaultState,
  ViewMode,
  SortMode,
} from "$lib/types";

// App state
export const appConfig = writable<AppConfig | null>(null);
export const vaultReady = writable(false);

// UI state
export const viewMode = writable<ViewMode>("all");
export const sortMode = writable<SortMode>("modified");
export const sidebarCollapsed = writable(false);
export const sidebarWidth = writable(220);
export const notelistWidth = writable(280);
export const searchQuery = writable("");
export const showCommandPalette = writable(false);
export const showSearch = writable(false);
export const showSettings = writable(false);
export const settingsTab = writable<string | null>(null);
export const showInfo = writable(false);
export const notebookIcons = writable<Record<string, string>>({});
export const quickAccessPaths = writable<string[]>([]);
export const collapsedNotebooks = writable<string[]>([]);

// Data
export const notebooks = writable<NotebookEntry[]>([]);
export const rootNoteCount = writable<number>(0);
export const notes = writable<NoteEntry[]>([]);
export const tags = writable<[string, number][]>([]);
export const activeNote = writable<NoteContent | null>(null);
export const activeNotePath = writable<string | null>(null);
export const activeNotebook = writable<NotebookEntry | null>(null);
export const activeTag = writable<string | null>(null);

// Mobile state
export const mobileView = writable<"sidebar" | "notelist" | "editor">(
  "sidebar",
);

// Editor state
export const editorDirty = writable(false);
export const sourceMode = writable(false);
export const focusMode = writable(false);
export const readOnly = writable(false);

// Theme
export const theme = writable<string>("system");

// Update state
export const updateAvailable = writable<{
  version: string;
  body?: string;
} | null>(null);
export const updateObj = writable<any>(null);
export const installType = writable<string>("native");
export const androidApkUrl = writable<string | null>(null);

export async function checkForUpdate() {
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    if (update) {
      updateAvailable.set({ version: update.version, body: update.body });
      updateObj.set(update);
    }
  } catch {
    // Silent fail — don't disrupt app startup
  }
}

function isNewerVersion(remote: string, local: string): boolean {
  const r = remote.split(".").map(Number);
  const l = local.split(".").map(Number);
  for (let i = 0; i < Math.max(r.length, l.length); i++) {
    const rv = r[i] || 0;
    const lv = l[i] || 0;
    if (rv > lv) return true;
    if (rv < lv) return false;
  }
  return false;
}

export async function checkForUpdateMobile() {
  try {
    const { getVersion } = await import("@tauri-apps/api/app");
    const currentVersion = await getVersion();
    const res = await fetch("https://helixnotes.com/latest.json");
    if (!res.ok) return;
    const data = await res.json();
    if (data.version && isNewerVersion(data.version, currentVersion)) {
      updateAvailable.set({ version: data.version, body: data.notes });
      const android = data.platforms?.["android-universal"];
      if (android?.url) {
        androidApkUrl.set(android.url);
      }
    }
  } catch {
    // Silent fail
  }
}

// Derived
export const sortedNotes = derived(
  [notes, sortMode, viewMode],
  ([$notes, $sortMode, $viewMode]) => {
    // Quick Access preserves stored order
    if ($viewMode === "quickaccess") return $notes;

    const pinned = $notes.filter((n) => n.meta.pinned);
    const unpinned = $notes.filter((n) => !n.meta.pinned);

    const sortFn = (a: NoteEntry, b: NoteEntry) => {
      switch ($sortMode) {
        case "title":
          return a.meta.title.localeCompare(b.meta.title);
        case "created":
          return (
            new Date(b.meta.created).getTime() -
            new Date(a.meta.created).getTime()
          );
        case "modified":
        default:
          return (
            new Date(b.meta.modified).getTime() -
            new Date(a.meta.modified).getTime()
          );
      }
    };

    return [...pinned.sort(sortFn), ...unpinned.sort(sortFn)];
  },
);

export const vaultState = derived(
  [
    activeNotePath,
    sidebarWidth,
    notelistWidth,
    sidebarCollapsed,
    collapsedNotebooks,
  ],
  ([
    $activeNotePath,
    $sidebarWidth,
    $notelistWidth,
    $sidebarCollapsed,
    $collapsedNotebooks,
  ]) => {
    return {
      last_open_note: $activeNotePath,
      sidebar_width: $sidebarWidth,
      notelist_width: $notelistWidth,
      sidebar_collapsed: $sidebarCollapsed,
      collapsed_notebooks: $collapsedNotebooks,
    } satisfies VaultState;
  },
);
