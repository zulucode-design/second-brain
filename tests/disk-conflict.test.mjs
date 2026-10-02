import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

async function importTypeScript(relativePath) {
  const source = await readFile(new URL(relativePath, import.meta.url), 'utf8');
  const { code } = await transformWithEsbuild(source, relativePath, { loader: 'ts', format: 'esm', target: 'esnext' });
  return import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
}

const { DiskConflictError, reportSaveFailure, resolveDiskConflict } = await importTypeScript('../src/lib/utils/document-lifecycle.ts');
const { SaveCoordinator } = await importTypeScript('../src/lib/utils/save-coordinator.ts');

function deferred() {
  let resolve;
  const promise = new Promise((res) => { resolve = res; });
  return { promise, resolve };
}

const PATH = '/vault/Areas/Plan.md';
const COPY = '/vault/Areas/Plan.sync-conflict-20261002-101010-ABCDEFG.md';

// An editor wired the way Editor.svelte wires it to save_note_or_preserve, after the note
// changed on disk: every save keeps the draft in the copy and fails with DiskConflictError.
function editor({ disk = { revision: 'disk', content: 'theirs' } } = {}) {
  const state = { body: 'A', copy: null, drafting: false, locks: 0, releases: 0, calls: [], gate: null, failPersist: null, failResolve: null };
  const coordinator = new SaveCoordinator({
    delayMs: 500,
    capture: () => ({ path: PATH, body: state.body }),
    persist: async (snapshot) => {
      state.calls.push(`persist ${snapshot.body}`);
      if (state.gate) await state.gate.promise;
      if (state.failPersist) throw state.failPersist;
      state.copy = snapshot.body;
      throw new DiskConflictError({ path: snapshot.path, documentVersion: snapshot.documentVersion, revision: snapshot.revision, copyPath: COPY, disk });
    },
    onDirtyChange: () => {},
    setTimer: () => 0,
    clearTimer: () => {},
  });
  coordinator.setDocument(PATH);
  const type = (body) => { state.body = body; coordinator.markDirty(); };
  const options = (choice) => ({
    choice,
    drafting: () => state.drafting,
    lock: async () => { state.locks += 1; return () => { state.releases += 1; }; },
    current: () => ({ path: PATH, documentVersion: coordinator.getDocumentVersion(), revision: coordinator.getRevision() }),
    flush: () => coordinator.flush(),
    resolve: async (copyPath, keep) => {
      state.calls.push(`resolve ${keep} with copy ${state.copy}`);
      assert.equal(copyPath, COPY);
      if (state.failResolve) throw state.failResolve;
    },
    read: async (path) => { state.calls.push(`read ${path}`); return { revision: 'new', content: state.copy }; },
    load: (path, content) => { state.calls.push(`load ${content.content}`); coordinator.setDocument(path, true); },
    close: () => { state.calls.push('close'); coordinator.setDocument(null, true); },
  });
  return { state, coordinator, type, options };
}

for (const [choice, keep] of [['mine', 'conflict'], ['disk', 'original']]) {
  test(`${choice}: newer typing joined to an older preserving save reaches the copy before anything is replaced`, async () => {
    const { state, coordinator, type, options } = editor();
    type('A');
    state.gate = deferred();
    const inFlight = coordinator.flush();
    await new Promise((resolve) => setImmediate(resolve));
    type('B');
    const resolving = resolveDiskConflict(options(choice));
    state.gate.resolve();
    state.gate = null;
    assert.equal((await inFlight).ok, false);
    assert.deepEqual(await resolving, { ok: true });
    assert.deepEqual(state.calls, ['persist A', 'persist B', `resolve ${keep} with copy B`, `read ${PATH}`, 'load B']);
    assert.equal(coordinator.isDirty(), false);
    assert.equal(state.releases, state.locks);
  });
}

test('close: proves the newest draft is in the copy, then closes without resolving or reading', async () => {
  const { state, coordinator, type, options } = editor({ disk: null });
  type('A');
  state.gate = deferred();
  const inFlight = coordinator.flush();
  await new Promise((resolve) => setImmediate(resolve));
  type('B');
  const resolving = resolveDiskConflict(options('close'));
  state.gate.resolve();
  state.gate = null;
  await inFlight;
  assert.deepEqual(await resolving, { ok: true });
  assert.deepEqual(state.calls, ['persist A', 'persist B', 'close']);
  assert.equal(coordinator.isDirty(), false);
});

test('taking the disk version of a note that is gone is refused, keeping the draft', async () => {
  const { state, coordinator, type, options } = editor({ disk: null });
  type('A');
  const result = await resolveDiskConflict(options('disk'));
  assert.equal(result.ok, false);
  assert.deepEqual(state.calls, ['persist A']);
  assert.equal(coordinator.isDirty(), true);
});

test('a save failure that kept nothing aborts with the editor still dirty', async () => {
  const { state, coordinator, type, options } = editor();
  type('A');
  state.failPersist = new Error('disk full');
  const result = await resolveDiskConflict(options('mine'));
  assert.equal(result.ok, false);
  assert.match(result.message, /disk full/);
  assert.deepEqual(state.calls, ['persist A']);
  assert.equal(coordinator.isDirty(), true);
  assert.equal(state.releases, state.locks);
});

test('an unexpected successful save aborts without replacing the editor', async () => {
  const { state, coordinator, options } = editor();
  const result = await resolveDiskConflict({ ...options('mine'), flush: async () => ({ ok: true, status: 'saved', revision: 1 }) });
  assert.equal(result.ok, false);
  assert.deepEqual(state.calls, []);
  assert.equal(coordinator.getDocumentVersion() > 0, true);
});

test('typing, a same-path reload, or a draft during the flush ends it with nothing replaced', async () => {
  const cases = {
    typing: (ctx) => ctx.type('C'),
    reload: (ctx) => ctx.coordinator.setDocument(PATH, true),
    draft: (ctx) => { ctx.state.drafting = true; },
  };
  for (const [name, interfere] of Object.entries(cases)) {
    const ctx = editor();
    ctx.type('A');
    ctx.state.gate = deferred();
    const resolving = resolveDiskConflict(ctx.options('mine'));
    await new Promise((resolve) => setImmediate(resolve));
    interfere(ctx);
    ctx.state.gate.resolve();
    const result = await resolving;
    assert.equal(result.ok, false, name);
    assert.ok(!ctx.state.calls.some((call) => call.startsWith('resolve') || call.startsWith('load')), name);
    assert.equal(ctx.state.releases, ctx.state.locks, name);
  }
});

test('a draft open before the choice refuses without taking the lock', async () => {
  const { state, type, options } = editor();
  type('A');
  state.drafting = true;
  const result = await resolveDiskConflict(options('mine'));
  assert.equal(result.ok, false);
  assert.equal(state.locks, 0);
  assert.deepEqual(state.calls, []);
});

test('a draft appearing during the read keeps the editor as it is', async () => {
  const { state, coordinator, type, options } = editor();
  type('A');
  const result = await resolveDiskConflict({
    ...options('mine'),
    read: async () => { state.drafting = true; return { revision: 'new', content: 'A' }; },
  });
  assert.equal(result.ok, false);
  assert.ok(!state.calls.some((call) => call.startsWith('load')));
  assert.equal(coordinator.isDirty(), true);
});

test('a failed resolve keeps the edits in a fresh copy and does not load', async () => {
  const { state, coordinator, type, options } = editor();
  type('A');
  state.failResolve = new Error('Conflict copy is gone');
  const result = await resolveDiskConflict(options('mine'));
  assert.equal(result.ok, false);
  assert.deepEqual(state.calls, ['persist A', 'resolve conflict with copy A', 'persist A']);
  assert.equal(coordinator.isDirty(), true);
});

test('a disk conflict blocks navigation, relocation and close without the generic save alert', () => {
  const alerts = [];
  globalThis.window = { alert: (message) => alerts.push(message) };
  const error = (value) => ({ ok: false, status: 'failed', revision: 1, error: value });
  const quiet = console.error;
  console.error = () => {};
  try {
    const receipt = { path: PATH, documentVersion: 1, revision: 1, copyPath: COPY, disk: null };
    assert.equal(reportSaveFailure('Closing the application', error(new DiskConflictError(receipt))), false);
    assert.deepEqual(alerts, []);
    assert.equal(reportSaveFailure('Closing the application', error(new Error('disk full'))), false);
    assert.equal(alerts.length, 1);
  } finally {
    console.error = quiet;
    delete globalThis.window;
  }
});

test('both windows and the editor route the conflict the same way', async () => {
  const source = (path) => readFile(new URL(`../${path}`, import.meta.url), 'utf8');
  const noteWindow = await source('src/lib/components/NoteWindow.svelte');
  assert.match(noteWindow, /if \(!\(result\.error instanceof DiskConflictError\)\) window\.alert\(/);
  const appLayout = await source('src/lib/components/AppLayout.svelte');
  assert.match(appLayout, /reportSaveFailure as reportSaveResult[^\n]*from '\$lib\/utils\/document-lifecycle'/);
  assert.match(appLayout, /return reportSaveResult\('Closing the application', await editor\?\.flushSave\(\)\);/);

  const editor = await source('src/lib/components/Editor.svelte');
  // Every editor save can preserve; a receipt is published only for the document it belongs to.
  assert.doesNotMatch(editor, /await saveNote\(/);
  assert.match(editor, /saveNoteOrPreserve\(path, meta, body, expectedRevision, diskConflict\?\.path === path \? diskConflict\.copyPath : null\)/);
  assert.match(editor, /if \(path === loadedPath && documentVersion === saveCoordinator\.getDocumentVersion\(\)\) \{/);
  // Explicit flushes reopen the dialog; resolution flushes the coordinator directly.
  assert.match(editor, /const result = await saveCoordinator\.flush\(\);\s*\/\/[^\n]*\n[^\n]*\n\s*if \(!result\.ok && result\.error instanceof DiskConflictError\) openDiskConflictDialog\(\);/);
  assert.match(editor, /flush: \(\) => saveCoordinator\.flush\(\),/);
  assert.match(editor, /function openDiskConflictDialog\(\) \{\s*if \(!diskConflict \|\| diskConflictBusy \|\| hasPendingDraft\(\)\) return;/);
  // Loading any document ends the conflict; a resolution keeps the editor frozen.
  assert.match(editor, /saveCoordinator\.setDocument\(path, true\);\s*clearDiskConflict\(\);/);
  assert.match(editor, /editor\.setEditable\(!shouldBeReadOnly && !diskConflictBusy, false\)/);
  assert.match(editor, /editor\.setEditable\(!ro && !shuttingDown && !preview && !resolving, false\)/);
  assert.equal([...editor.matchAll(/\|\| \$holdingPreview \|\| diskConflictBusy\}/g)].length, 2);
  assert.match(editor, /readonly=\{\$readOnly \|\| diskConflictBusy\}/);
});

test('a draft opened while the lock is being taken ends the choice before any flush', async () => {
  const { state, coordinator, type, options } = editor();
  type('A');
  const lock = deferred();
  const resolving = resolveDiskConflict({
    ...options('mine'),
    lock: async () => { state.locks += 1; await lock.promise; return () => { state.releases += 1; }; },
  });
  await new Promise((resolve) => setImmediate(resolve));
  state.drafting = true;
  lock.resolve();
  const result = await resolving;
  assert.equal(result.ok, false);
  assert.deepEqual(state.calls, []);
  assert.equal(coordinator.isDirty(), true);
  assert.equal(state.releases, 1);
});

test('the dialog keeps focus and keys while a choice is applied, and nothing reopens editing', async () => {
  const source = (path) => readFile(new URL(`../${path}`, import.meta.url), 'utf8');
  const noteWindow = await source('src/lib/components/NoteWindow.svelte');
  // Navigation in a note window reports the conflict through the dialog, not an alert.
  assert.match(noteWindow, /onSaveFailure: \(error\) => \{[\s\S]{0,200}if \(!\(error instanceof DiskConflictError\)\) window\.alert\(/);
  const editor = await source('src/lib/components/Editor.svelte');
  // Resolve… must not take focus from an uncommitted title, which would commit it as a rename.
  assert.match(editor, /onmousedown=\{\(e\) => e\.preventDefault\(\)\} onclick=\{resolveDiskConflictFromBar\}/);
  // No key leaves the dialog for the window's shortcuts, and focus stays in it while busy.
  assert.match(editor, /function trapDiskConflictFocus\(event: KeyboardEvent\) \{\s*event\.stopPropagation\(\);/);
  assert.match(editor, /diskConflictModal\?\.focus\(\);\s*diskConflictBusy = true;/);
  // An editor recreated during a choice (a mode switch) starts frozen; source keys do nothing.
  assert.match(editor, /editable: !\$readOnly && !diskConflictBusy,/);
  assert.equal([...editor.matchAll(/onkeydown=\{\(e\) => \{\s*if \(diskConflictBusy\) \{ e\.preventDefault\(\); return; \}\s*if \(handleSourceCtrlEnd\(e\)\) return;/g)].length, 2);
});
