import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const root = new URL('../', import.meta.url);

const source = await readFile(
  new URL('src/lib/utils/async-view-state.ts', root),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'async-view-state.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { asyncViewState, describeLoadFailure } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

test('a load in flight renders as loading, never as empty', () => {
  assert.deepEqual(asyncViewState({ status: 'loading', itemCount: 0 }), {
    kind: 'loading'
  });
  // A refresh over existing content is still a load, but content stays on screen.
  assert.deepEqual(asyncViewState({ status: 'loading', itemCount: 7 }), {
    kind: 'content'
  });
});

test('a completed load with no items is a genuine empty state', () => {
  assert.deepEqual(asyncViewState({ status: 'loaded', itemCount: 0 }), {
    kind: 'empty'
  });
});

test('a completed load with items renders content', () => {
  assert.deepEqual(asyncViewState({ status: 'loaded', itemCount: 3 }), {
    kind: 'content'
  });
});

test('a failed load never renders as empty and always offers retry', () => {
  const state = asyncViewState({
    status: 'failed',
    itemCount: 0,
    error: new Error('vault unreadable')
  });
  assert.equal(state.kind, 'failed');
  assert.equal(state.retry, true);
  assert.match(state.message, /vault unreadable/);
});

test('a failure that arrives after content still reports the failure', () => {
  // Silently keeping stale rows would tell the user the backend is fine when it is not.
  const state = asyncViewState({
    status: 'failed',
    itemCount: 5,
    error: new Error('index closed')
  });
  assert.equal(state.kind, 'failed');
  assert.equal(state.retry, true);
});

test('failure messages stay readable whatever the backend threw', () => {
  assert.match(describeLoadFailure(new Error('boom')), /boom/);
  assert.match(describeLoadFailure('plain string rejection'), /plain string rejection/);
  // Tauri command rejections arrive as plain strings or as objects with a message.
  assert.match(describeLoadFailure({ message: 'command failed' }), /command failed/);
  const fallback = describeLoadFailure(undefined);
  assert.equal(typeof fallback, 'string');
  assert.ok(fallback.length > 0, 'an unknown failure still needs something to show');
});

test('failure messages never collapse to an empty or placeholder string', () => {
  for (const thrown of [null, undefined, '', {}, 0]) {
    const message = describeLoadFailure(thrown);
    assert.ok(
      message.trim().length > 0,
      `describeLoadFailure(${JSON.stringify(thrown)}) must produce something visible`
    );
    assert.doesNotMatch(message, /undefined|\[object Object\]/);
  }
});

// ── Source policy: the views must not reintroduce silent failure ──

const views = ['src/lib/components/TasksView.svelte', 'src/lib/components/GraphView.svelte'];

test('the Tasks and Graph views resolve their rendering through the shared state helper', async () => {
  for (const path of views) {
    const contents = await readFile(new URL(path, root), 'utf8');
    assert.match(
      contents,
      /from ['"]\$lib\/utils\/async-view-state['"]/,
      `${path} must classify loading, empty, and failed through the shared helper`
    );
    assert.match(
      contents,
      /asyncViewState\(/,
      `${path} must call asyncViewState to decide what to render`
    );
  }
});

test('a caught load failure is recorded as failure rather than logged and dropped', async () => {
  for (const path of views) {
    const contents = await readFile(new URL(path, root), 'utf8');
    const catchBlocks = contents.match(/catch\s*\([^)]*\)\s*\{[^}]*\}/g) ?? [];
    const loadCatches = catchBlocks.filter((block) => /console\.error/.test(block));
    for (const block of loadCatches) {
      assert.match(
        block,
        /loadError|status\s*=\s*'failed'/,
        `${path} logs a load failure without recording it: ${block}`
      );
    }
  }
});

test('the views offer a retry control for a failed load', async () => {
  for (const path of views) {
    const contents = await readFile(new URL(path, root), 'utf8');
    assert.match(
      contents,
      /Retry|retry\(/,
      `${path} must let the user retry a failed load`
    );
  }
});


// ── Source policy: a rejected task edit must be visible, not console-only ──

test('a failed task mutation is shown to the user instead of only logged', async () => {
  const contents = await readFile(
    new URL('src/lib/components/AppLayout.svelte', root),
    'utf8'
  );
  const mutateTask = contents.slice(
    contents.indexOf('async function mutateTask('),
    contents.indexOf('async function toggleTask(')
  );
  assert.ok(mutateTask.length > 0, 'mutateTask must remain the shared task mutation path');
  assert.match(
    mutateTask,
    /showToast\(/,
    'a rejected task edit (impossible due date, moved note) must surface a visible message'
  );
  const catchBlock = mutateTask.slice(mutateTask.indexOf('} catch (error) {'));
  assert.match(
    catchBlock,
    /describeLoadFailure\(error\)/,
    'the message must carry the backend reason, not a generic placeholder'
  );
});
