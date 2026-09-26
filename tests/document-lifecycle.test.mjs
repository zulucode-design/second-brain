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

const { EditorMutationBarrier, saveForCurrentDocument } = await importTypeScript('../src/lib/utils/editor-mutation-barrier.ts');
const { relocateDocument, runSaveGatedAction } = await importTypeScript('../src/lib/utils/document-lifecycle.ts');
const { SerializedNavigationController } = await importTypeScript('../src/lib/utils/navigation-controller.ts');
const { GenerationGate } = await importTypeScript('../src/lib/utils/generation-gate.ts');
const { runActiveDocumentMutation } = await importTypeScript('../src/lib/utils/document-mutation.ts');

function deferred() {
  let resolve;
  const promise = new Promise((res) => { resolve = res; });
  return { promise, resolve };
}

const clean = { ok: true, status: 'clean', revision: 0 };

test('mutation preparation waits for an existing producer, rejects new work, and exposes its edit to the following flush', async () => {
  const barrier = new EditorMutationBarrier();
  barrier.setDocument('/vault/one.md');
  const work = deferred();
  let revision = 0;
  assert.equal(barrier.start(async (identity) => {
    await work.promise;
    if (barrier.isCurrent(identity)) revision += 1;
  }), true);

  let lockResolved = false;
  const lock = barrier.lockAndDrain().then((release) => {
    lockResolved = true;
    return release;
  });
  await Promise.resolve();
  assert.equal(lockResolved, false);
  assert.equal(barrier.start(async () => { revision += 100; }), false);

  work.resolve();
  const release = await lock;
  assert.equal(revision, 1);
  const flushedRevision = revision;
  assert.equal(flushedRevision, 1, 'close/navigation flush sees the producer revision after drain');
  release();
  assert.equal(barrier.start(async () => {}), true);
});

test('a producer validates its originating document before insertion, including same-path reloads', async () => {
  for (const replacementPath of ['/vault/two.md', '/vault/one.md']) {
    const barrier = new EditorMutationBarrier();
    barrier.setDocument('/vault/one.md');
    const work = deferred();
    let insertions = 0;
    barrier.start(async (identity) => {
      await work.promise;
      if (barrier.isCurrent(identity)) insertions += 1;
    });
    barrier.setDocument(replacementPath);
    work.resolve();
    await barrier.drain();
    assert.equal(insertions, 0);
  }
});

test('saved attachment is reported if a note switch or same-path reload cancels insertion', async () => {
  for (const replacementPath of ['/vault/two.md', '/vault/one.md']) {
    const barrier = new EditorMutationBarrier();
    barrier.setDocument('/vault/one.md');
    const identity = barrier.capture();
    const pendingSave = deferred();
    const inserted = [];
    const cancelled = [];
    const operation = saveForCurrentDocument(
      () => barrier.isCurrent(identity),
      () => pendingSave.promise,
      (path) => { inserted.push(path); return true; },
      (saved) => cancelled.push(saved)
    );
    barrier.setDocument(replacementPath);
    pendingSave.resolve('.helixnotes/attachments/file.txt');
    await operation;
    assert.deepEqual(inserted, []);
    assert.deepEqual(cancelled, [true], 'the saved file must be reported');
  }
});

test('attachment insertion succeeds only while its document is current', async () => {
  const barrier = new EditorMutationBarrier();
  barrier.setDocument('/vault/one.md');
  const identity = barrier.capture();
  const inserted = [];
  const cancelled = [];
  await saveForCurrentDocument(
    () => barrier.isCurrent(identity),
    async () => '.helixnotes/attachments/file.txt',
    (path) => { inserted.push(path); return true; },
    (saved) => cancelled.push(saved)
  );
  assert.deepEqual(inserted, ['.helixnotes/attachments/file.txt']);
  assert.deepEqual(cancelled, []);
});

test('a rejected editor insertion reports an already saved attachment', async () => {
  const cancelled = [];
  await saveForCurrentDocument(
    () => true,
    async () => '.helixnotes/attachments/file.txt',
    () => false,
    (saved) => cancelled.push(saved)
  );
  assert.deepEqual(cancelled, [true]);
});

test('active relocation never mutates the filesystem when save fails or identity changed', async () => {
  for (const currentPath of ['/vault/one.md', '/vault/other.md']) {
    let mutations = 0;
    let rebases = 0;
    const result = await relocateDocument({
      expectedPath: '/vault/one.md',
      currentPath: () => currentPath,
      prepare: async () => () => {},
      flush: async () => ({ ok: false, status: 'failed', revision: 1, error: new Error('conflict') }),
      mutate: async () => { mutations += 1; return { path: '/vault/moved.md', note: { revision: 'new' }, warnings: [] }; },
      rebase: () => { rebases += 1; }
    });
    assert.equal(result, null);
    assert.equal(mutations, 0);
    assert.equal(rebases, 0);
  }
});

test('active relocation orders flush, authoritative mutation, and atomic rebase without a second read', async () => {
  const order = [];
  let path = '/vault/one.md';
  const result = await relocateDocument({
    expectedPath: path,
    currentPath: () => path,
    prepare: async () => { order.push('prepare'); return () => order.push('release'); },
    flush: async () => { order.push('flush'); return clean; },
    mutate: async () => {
      order.push('mutate');
      return { path: '/vault/moved.md', note: { revision: 'new', content: 'committed' }, warnings: ['index repair'] };
    },
    rebase: (_oldPath, newPath, note) => { order.push(`rebase:${note.revision}`); path = newPath; }
  });
  assert.equal(result, '/vault/moved.md');
  assert.deepEqual(order, ['prepare', 'flush', 'mutate', 'rebase:new', 'release']);
});

test('flush-dependent delayed work cannot self-drain and cannot apply after close locks admission', async () => {
  const barrier = new EditorMutationBarrier();
  barrier.setDocument('/vault/one.md');
  const prework = deferred();
  let mutations = 0;
  const operation = barrier.runAfter(() => prework.promise, () => { mutations += 1; });
  const lock = barrier.lockAndDrain();
  const release = await lock;
  prework.resolve('ready');
  assert.equal(await operation, false);
  assert.equal(mutations, 0);
  release();
});

test('vault-switch decision gate does not tear down the editor after save rejection', async () => {
  let unmounted = false;
  const switched = await runSaveGatedAction(
    async () => false,
    async () => { unmounted = true; return true; }
  );
  assert.equal(switched, false);
  assert.equal(unmounted, false);
});

test('secondary navigation save rejection blocks read and commit', async () => {
  let reads = 0;
  let commits = 0;
  let dirty = true;
  const controller = new SerializedNavigationController({
    isBlocked: () => false,
    prepare: async () => () => {},
    flush: async () => ({ ok: false, status: 'failed', revision: 1, error: new Error('CAS conflict') }),
    read: async () => { reads += 1; return {}; },
    commit: () => { commits += 1; dirty = false; return true; }
  });
  assert.equal(await controller.navigate('/vault/two.md'), 'save-failed');
  assert.equal(reads, 0);
  assert.equal(commits, 0);
  assert.equal(dirty, true);
});

test('secondary navigation performs a final flush before committing', async () => {
  const order = [];
  let flushes = 0;
  const controller = new SerializedNavigationController({
    isBlocked: () => false,
    prepare: async () => { order.push('prepare'); return () => order.push('release'); },
    flush: async () => { order.push(`flush-${++flushes}`); return clean; },
    read: async () => { order.push('read'); return { revision: 'destination' }; },
    commit: () => { order.push('commit'); return true; }
  });
  assert.equal(await controller.navigate('/vault/two.md'), 'navigated');
  assert.deepEqual(order, ['prepare', 'flush-1', 'read', 'flush-2', 'commit', 'release']);
});


test('startup generation rejects late restoration after activity and permanently after unmount', () => {
  const gate = new GenerationGate();
  const restore = gate.capture();
  gate.invalidate();
  assert.equal(gate.isCurrent(restore), false);
  const lifetime = gate.capture();
  gate.cancel();
  assert.equal(gate.isCurrent(lifetime), false);
  assert.equal(gate.isCurrent(gate.capture()), false);
});


test('active task mutation holds its lock and cannot replace a navigation winner', async () => {
  let path = '/vault/one.md';
  let released = false;
  let commits = 0;
  const backend = deferred();
  const operation = runActiveDocumentMutation({
    expectedPath: path,
    currentPath: () => path,
    isBlocked: () => false,
    prepare: async () => () => { released = true; },
    flush: async () => clean,
    mutate: () => backend.promise,
    commit: () => { commits += 1; }
  });
  await new Promise((resolve) => setImmediate(resolve));
  path = '/vault/two.md';
  backend.resolve({ revision: 'task-write' });
  assert.equal(await operation, false);
  assert.equal(commits, 0);
  assert.equal(released, true);
});

test('a reported relocation tells the user when another note opened first or the mutation failed', async () => {
  const { relocateReported: relocate } = await importTypeScript('../src/lib/utils/document-lifecycle.ts');
  const base = {
    expectedPath: '/vault/one.md',
    reason: 'Renaming the note',
    prepare: async () => () => {},
    flush: async () => clean,
    rebase: () => {},
  };
  const reports = [];
  const report = (message) => reports.push(message);
  const errors = console.error;
  console.error = () => {};
  try {
    assert.equal(await relocate({ ...base, report, currentPath: () => '/vault/other.md', mutate: async () => { throw new Error('must not run'); } }), null);
    assert.equal(await relocate({
      ...base, report, currentPath: () => '/vault/one.md', mutate: async () => { throw new Error('name taken'); },
    }), null);
    const moved = await relocate({
      ...base, report, currentPath: () => '/vault/one.md', mutate: async () => ({ path: '/vault/two.md', note: { revision: 'r' }, warnings: [] }),
    });
    assert.equal(moved, '/vault/two.md');
  } finally {
    console.error = errors;
  }
  assert.deepEqual(reports, [
    'Renaming the note was not applied: another note opened first.',
    'Renaming the note failed: Error: name taken',
  ]);
});
