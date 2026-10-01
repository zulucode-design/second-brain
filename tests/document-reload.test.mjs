import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

async function importTypeScript(relativePath) {
  const source = await readFile(new URL(relativePath, import.meta.url), 'utf8');
  const { code } = await transformWithEsbuild(source, relativePath, {
    loader: 'ts',
    format: 'esm',
    target: 'esnext'
  });
  return import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
}

const { lockAfterReload, reloadCleanDocument } = await importTypeScript('../src/lib/utils/document-reload.ts');
const { EditorMutationBarrier } = await importTypeScript('../src/lib/utils/editor-mutation-barrier.ts');
const { SerializedNavigationController } = await importTypeScript('../src/lib/utils/navigation-controller.ts');
const { applyConflictChoice } = await importTypeScript('../src/lib/utils/conflict-choice.ts');

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

// An editor showing `path` at `revision`, with the guards the components check.
function harness({ path = '/vault/a.md', revision = 'r1', disk = { revision: 'r2', content: 'new' } } = {}) {
  const editor = { path, revision, dirty: false, closing: false, preview: false, locked: 0, released: 0, committed: [] };
  const read = deferred();
  const options = {
    capture: () => (editor.dirty || editor.closing || editor.preview || !editor.path ? null : { path: editor.path, revision: editor.revision }),
    read: () => read.promise,
    lock: async () => { editor.locked += 1; return () => { editor.released += 1; }; },
    stillValid: (captured) => !editor.dirty && !editor.closing && !editor.preview
      && editor.path === captured.path && editor.revision === captured.revision,
    commit: (loadedPath, content) => {
      editor.committed.push({ path: loadedPath, content: content.content });
      editor.revision = content.revision;
    },
  };
  return { editor, options, finishRead: () => read.resolve(disk), failRead: (error) => read.reject(error) };
}

test('a clean editor takes the changed note from disk, and later saves start from its revision', async () => {
  const { editor, options, finishRead } = harness();
  const reloading = reloadCleanDocument(options);
  finishRead();
  assert.equal(await reloading, true);
  assert.deepEqual(editor.committed, [{ path: '/vault/a.md', content: 'new' }]);
  assert.equal(editor.revision, 'r2');
  assert.equal(editor.released, editor.locked);
});

test('a frontmatter-only change still reloads, because the revision moved', async () => {
  const { editor, options, finishRead } = harness({ disk: { revision: 'r2', content: 'same body' } });
  const reloading = reloadCleanDocument(options);
  finishRead();
  assert.equal(await reloading, true);
  assert.equal(editor.revision, 'r2');
});

test('the same revision on disk, including the echo of our own save, changes nothing', async () => {
  const { editor, options, finishRead } = harness({ disk: { revision: 'r1', content: 'old' } });
  const reloading = reloadCleanDocument(options);
  finishRead();
  assert.equal(await reloading, false);
  assert.deepEqual(editor.committed, []);
  assert.equal(editor.locked, 0);
});

test('a dirty editor is never replaced, before or during the read', async () => {
  const before = harness();
  before.editor.dirty = true;
  assert.equal(await reloadCleanDocument(before.options), false);

  const during = harness();
  const reloading = reloadCleanDocument(during.options);
  during.editor.dirty = true;
  during.finishRead();
  assert.equal(await reloading, false);
  assert.deepEqual(during.editor.committed, []);
});

test('an edit that autosaved during the read moved the revision, so the read is stale', async () => {
  const { editor, options, finishRead } = harness();
  const reloading = reloadCleanDocument(options);
  editor.dirty = true;
  editor.revision = 'r-own-save';
  editor.dirty = false;
  finishRead();
  assert.equal(await reloading, false);
  assert.deepEqual(editor.committed, []);
  assert.equal(editor.revision, 'r-own-save');
});

test('navigating away or starting a close during the read cancels the reload', async () => {
  const navigated = harness();
  const reloadingNavigated = reloadCleanDocument(navigated.options);
  navigated.editor.path = '/vault/b.md';
  navigated.finishRead();
  assert.equal(await reloadingNavigated, false);

  const closing = harness();
  const reloadingClosing = reloadCleanDocument(closing.options);
  closing.editor.closing = true;
  closing.finishRead();
  assert.equal(await reloadingClosing, false);
  assert.deepEqual([...navigated.editor.committed, ...closing.editor.committed], []);
});

test('a change while waiting for the lock is checked again under it', async () => {
  const { editor, options, finishRead } = harness();
  const lock = deferred();
  options.lock = async () => { editor.locked += 1; await lock.promise; return () => { editor.released += 1; }; };
  const reloading = reloadCleanDocument(options);
  finishRead();
  await new Promise((resolve) => setImmediate(resolve));
  editor.dirty = true;
  lock.resolve();
  assert.equal(await reloading, false);
  assert.deepEqual(editor.committed, []);
  assert.equal(editor.released, 1);
});

test('a note that cannot be read keeps what the editor shows', async () => {
  const { editor, options, failRead } = harness();
  const warn = console.warn;
  console.warn = () => {};
  try {
    const reloading = reloadCleanDocument(options);
    failRead(new Error('Note not found'));
    assert.equal(await reloading, false);
  } finally {
    console.warn = warn;
  }
  assert.deepEqual(editor.committed, []);
  assert.equal(editor.locked, 0);
});

test('the lock is released when the commit throws', async () => {
  const { editor, options, finishRead } = harness();
  options.commit = () => { throw new Error('editor gone'); };
  const reloading = reloadCleanDocument(options);
  finishRead();
  await assert.rejects(reloading, /editor gone/);
  assert.equal(editor.released, 1);
});

test('a reload queued with navigation waits its turn instead of colliding with its lock', async () => {
  const barrier = new EditorMutationBarrier();
  barrier.setDocument('/vault/a.md');
  const navigationRead = deferred();
  const controller = new SerializedNavigationController({
    isBlocked: () => false,
    prepare: () => barrier.lockAndDrain(),
    flush: async () => ({ ok: true, status: 'clean', revision: 0 }),
    read: () => navigationRead.promise,
    commit: () => true,
  });
  const navigating = controller.navigate('/vault/b.md');
  await new Promise((resolve) => setImmediate(resolve));
  // Unqueued, this second lock throws "preparation is already in progress".
  await assert.rejects(barrier.lockAndDrain(), /already in progress/);
  const reloading = controller.enqueue(() => reloadCleanDocument({
    capture: () => ({ path: '/vault/b.md', revision: 'r1' }),
    read: async () => ({ revision: 'r2', content: 'new' }),
    lock: () => barrier.lockAndDrain(),
    stillValid: () => true,
    commit: () => {},
  }));
  navigationRead.resolve({ revision: 'r1' });
  assert.equal(await navigating, 'navigated');
  assert.equal(await reloading, true);
});

test('a close waits out a reload that holds the lock, then takes it', async () => {
  const barrier = new EditorMutationBarrier();
  barrier.setDocument('/vault/a.md');
  const reloadLock = await barrier.lockAndDrain();
  const reloadDone = deferred();
  const reload = reloadDone.promise.finally(reloadLock);
  const closing = lockAfterReload(reload, () => barrier.lockAndDrain(), () => true);
  reloadDone.reject(new Error('the reload failed'));
  const held = await closing;
  assert.ok(held?.release);
  held.release();
});

test('a close released while it waited takes no lock, and one released while locking gives it back', async () => {
  let locks = 0;
  let releases = 0;
  const lock = async () => { locks += 1; return () => { releases += 1; }; };

  let closing = true;
  const reload = deferred();
  const waiting = lockAfterReload(reload.promise, lock, () => closing);
  closing = false;
  reload.resolve();
  assert.equal(await waiting, null);
  assert.equal(locks, 0);

  closing = true;
  const slowLock = deferred();
  const locking = lockAfterReload(Promise.resolve(), async () => { await slowLock.promise; return lock(); }, () => closing);
  await new Promise((resolve) => setImmediate(resolve));
  closing = false;
  slowLock.resolve();
  assert.equal(await locking, null);
  assert.equal(releases, 1);
});

test('a conflict choice reloads the open note at once, without waiting for the conflict list', async () => {
  const list = deferred();
  let reloads = 0;
  const choosing = applyConflictChoice(async () => {}, () => { reloads += 1; }, () => list.promise);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(reloads, 1);
  list.resolve(['remaining']);
  assert.deepEqual(await choosing, ['remaining']);
});

test('a failed conflict choice still reloads, then reports the failure without listing', async () => {
  let reloads = 0;
  let listed = 0;
  await assert.rejects(
    applyConflictChoice(async () => { throw new Error('Could not reconcile'); }, () => { reloads += 1; }, async () => { listed += 1; }),
    /Could not reconcile/,
  );
  assert.equal(reloads, 1);
  assert.equal(listed, 0);
});

test('both windows treat an uncommitted note or callout title like unsaved text', async () => {
  const source = (path) => readFile(new URL(`../${path}`, import.meta.url), 'utf8');
  const editor = await source('src/lib/components/Editor.svelte');
  assert.match(editor, /export function hasPendingDraft\(\)[\s\S]{0,160}pendingCalloutTitles\.size > 0 \|\| \(!!titleInput && !!\$activeNote && titleInput\.value !== \$activeNote\.meta\.title\)/);
  // A callout title is pending from its first keystroke until it commits or its node goes.
  assert.match(editor, /addEventListener\('input', \(\) => \{ titleDirty = true; pendingCalloutTitles\.add\(titleInput\); \}\)/);
  assert.match(editor, /const commitTitle = \(\) => \{[\s\S]{0,160}pendingCalloutTitles\.delete\(titleInput\);/);
  assert.match(editor, /destroy\(\) \{\s*pendingCalloutTitles\.delete\(titleInput\);/);
  for (const path of ['src/lib/components/AppLayout.svelte', 'src/lib/components/NoteWindow.svelte']) {
    assert.match(await source(path), /const reloadable = [^;]*!\$editorDirty && !current\.hasPendingDraft\(\)/, path);
  }
});
