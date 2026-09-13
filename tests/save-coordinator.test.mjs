import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/save-coordinator.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'save-coordinator.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { SaveCoordinator } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function harness(persist = async () => {}) {
  let path = '/vault/one.md';
  let body = 'one';
  let dirty = false;
  let timer = null;
  const snapshots = [];
  const coordinator = new SaveCoordinator({
    delayMs: 500,
    capture: () => ({ path, body }),
    persist: async (snapshot) => {
      snapshots.push(snapshot);
      await persist(snapshot);
    },
    onDirtyChange: (next) => { dirty = next; },
    setTimer: (callback) => { timer = callback; return 1; },
    clearTimer: () => { timer = null; }
  });
  coordinator.setDocument(path);
  return {
    coordinator,
    snapshots,
    get dirty() { return dirty; },
    get timer() { return timer; },
    fireTimer() { const callback = timer; timer = null; callback?.(); },
    edit(nextBody) { body = nextBody; coordinator.markDirty(); },
    changeDocument(nextPath, nextBody, forceReset = false) {
      path = nextPath;
      body = nextBody;
      coordinator.setDocument(nextPath, forceReset);
    }
  };
}

test('immediate navigation cancels debounce and waits for the captured save', async () => {
  const write = deferred();
  const h = harness(() => write.promise);
  h.edit('latest body');
  assert.ok(h.timer);

  let navigated = false;
  const navigation = h.coordinator.flush().then((result) => {
    if (result.ok) navigated = true;
    return result;
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(h.timer, null);
  assert.equal(navigated, false);
  assert.equal(h.snapshots.length, 1);
  assert.equal(h.snapshots[0].path, '/vault/one.md');
  assert.equal(h.snapshots[0].body, 'latest body');

  write.resolve();
  assert.equal((await navigation).ok, true);
  assert.equal(navigated, true);
  assert.equal(h.dirty, false);
});

test('save rejection blocks navigation, keeps dirty, and remains retryable', async () => {
  let attempt = 0;
  const h = harness(async () => {
    attempt += 1;
    if (attempt === 1) throw new Error('disk full');
  });
  h.edit('must survive');

  const failed = await h.coordinator.flush();
  assert.equal(failed.ok, false);
  assert.match(String(failed.error), /disk full/);
  assert.equal(h.dirty, true);

  const retried = await h.coordinator.flush();
  assert.equal(retried.ok, true);
  assert.equal(h.snapshots.length, 2);
  assert.equal(h.dirty, false);
});

test('close during debounce drains the pending revision before resolving', async () => {
  const write = deferred();
  const h = harness(() => write.promise);
  h.edit('pending debounce');

  let closed = false;
  const close = h.coordinator.flush().then((result) => {
    closed = result.ok;
    return result;
  });
  await Promise.resolve();

  assert.equal(closed, false);
  assert.equal(h.timer, null);
  write.resolve();
  await close;
  assert.equal(closed, true);
});

test('close during an in-flight autosave joins the same serialization tail', async () => {
  const write = deferred();
  const h = harness(() => write.promise);
  h.edit('in flight');
  h.fireTimer();
  await Promise.resolve();

  let closed = false;
  const close = h.coordinator.flush().then((result) => {
    closed = result.ok;
    return result;
  });
  await Promise.resolve();
  assert.equal(closed, false);
  assert.equal(h.snapshots.length, 1);

  write.resolve();
  await close;
  assert.equal(closed, true);
  assert.equal(h.snapshots.length, 1);
});

test('a newer same-path revision stays dirty and is drained after an older save', async () => {
  const first = deferred();
  const second = deferred();
  let attempt = 0;
  const h = harness(() => (attempt++ === 0 ? first.promise : second.promise));
  h.edit('revision one');
  const drain = h.coordinator.flush();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(h.snapshots[0].revision, 1);

  h.edit('revision two');
  first.resolve();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(h.dirty, true);
  assert.equal(h.snapshots.length, 2);
  assert.equal(h.snapshots[1].revision, 2);
  assert.equal(h.snapshots[1].body, 'revision two');

  second.resolve();
  assert.equal((await drain).ok, true);
  assert.equal(h.dirty, false);
});

test('an old-path completion cannot clear a newer document revision', async () => {
  const first = deferred();
  const second = deferred();
  const h = harness((snapshot) => snapshot.path.endsWith('one.md') ? first.promise : second.promise);
  h.edit('old document edit');
  const drain = h.coordinator.flush();
  await Promise.resolve();

  h.changeDocument('/vault/two.md', 'new document');
  h.edit('new document edit');
  first.resolve();
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(h.dirty, true);
  assert.equal(h.snapshots.length, 2);
  assert.equal(h.snapshots[1].path, '/vault/two.md');
  second.resolve();
  await drain;
  assert.equal(h.dirty, false);
});


test('a metadata-only edit advances the revision and persists the updated snapshot', async () => {
  let meta = { pinned: false };
  const snapshots = [];
  const coordinator = new SaveCoordinator({
    delayMs: 500,
    capture: () => ({ path: '/vault/one.md', body: 'unchanged', meta: { ...meta } }),
    persist: async (snapshot) => { snapshots.push(snapshot); },
    onDirtyChange: () => {},
    setTimer: () => 1,
    clearTimer: () => {}
  });
  coordinator.setDocument('/vault/one.md');

  meta = { pinned: true };
  coordinator.markDirty();
  const result = await coordinator.flush();

  assert.equal(result.ok, true);
  assert.equal(snapshots.length, 1);
  assert.equal(snapshots[0].revision, 1);
  assert.deepEqual(snapshots[0].meta, { pinned: true });
});


test('an edit arriving during preparation is prepared again before capture', async () => {
  const firstPrepare = deferred();
  let body = 'first revision';
  let preparedBody = '';
  let prepareCalls = 0;
  const snapshots = [];
  const coordinator = new SaveCoordinator({
    delayMs: 500,
    prepare: async () => {
      const candidate = body;
      prepareCalls += 1;
      if (prepareCalls === 1) await firstPrepare.promise;
      preparedBody = candidate;
    },
    capture: () => ({ path: '/vault/one.md', body: preparedBody }),
    persist: async (snapshot) => { snapshots.push(snapshot); },
    onDirtyChange: () => {},
    setTimer: () => 1,
    clearTimer: () => {}
  });
  coordinator.setDocument('/vault/one.md');
  coordinator.markDirty();
  const drain = coordinator.flush();
  await new Promise((resolve) => setImmediate(resolve));

  body = 'new paste requiring preparation';
  coordinator.markDirty();
  firstPrepare.resolve();

  assert.equal((await drain).ok, true);
  assert.equal(prepareCalls, 2);
  assert.equal(snapshots.length, 1);
  assert.equal(snapshots[0].revision, 2);
  assert.equal(snapshots[0].body, 'new paste requiring preparation');
});


test('a capture path that diverges from coordinator identity fails once without persisting', async () => {
  // Model the real list/sidebar bug: the visible capture path changes while the
  let capturedPath = '/vault/one.md';
  let dirty = false;
  let persistCalls = 0;
  const coordinator = new SaveCoordinator({
    delayMs: 500,
    capture: () => ({ path: capturedPath, body: 'must remain open' }),
    persist: async () => { persistCalls += 1; },
    onDirtyChange: (next) => { dirty = next; },
    setTimer: () => 1,
    clearTimer: () => {}
  });
  coordinator.setDocument('/vault/one.md');
  coordinator.markDirty();
  capturedPath = '/vault/moved.md';

  const result = await coordinator.flush();
  assert.equal(result.ok, false);
  assert.match(String(result.error), /Save invariant failed/);
  assert.equal(persistCalls, 0);
  assert.equal(dirty, true);
});


test('a coordinator-owned metadata save adopts the returned revision for the next write', async () => {
  let expectedRevision = 'read-revision';
  let body = 'body';
  let pinned = false;
  const seenRevisions = [];
  const coordinator = new SaveCoordinator({
    delayMs: 500,
    capture: () => ({ path: '/vault/one.md', body, pinned, expectedRevision }),
    persist: async (snapshot) => {
      seenRevisions.push(snapshot.expectedRevision);
      expectedRevision = `saved-${snapshot.revision}`;
    },
    onDirtyChange: () => {},
    setTimer: () => 1,
    clearTimer: () => {}
  });
  coordinator.setDocument('/vault/one.md');

  body = 'dirty body';
  coordinator.markDirty();
  assert.equal((await coordinator.flush()).ok, true);
  pinned = true;
  coordinator.markDirty();
  assert.equal((await coordinator.flush()).ok, true);

  assert.deepEqual(seenRevisions, ['read-revision', 'saved-1']);
});
