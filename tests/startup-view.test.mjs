import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/startup-view.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'startup-view.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { normalizeStartupView, resolveStartupTarget } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

const base = {
  startupView: 'quickaccess',
  restoreLastSession: true,
  lastViewMode: 'all',
  lastNotebook: null,
  lastTag: null
};

test('a legacy empty root notebook resumes at Needs a category', () => {
  assert.deepEqual(resolveStartupTarget({
    ...base,
    lastViewMode: 'notebook',
    lastNotebook: ''
  }), { mode: 'unfiled' });
});

test('real notebooks and current list views still restore normally', () => {
  assert.deepEqual(resolveStartupTarget({
    ...base,
    lastViewMode: 'notebook',
    lastNotebook: 'Projects/Launch'
  }), { mode: 'notebook', notebookPath: 'Projects/Launch' });
  assert.deepEqual(resolveStartupTarget({ ...base, lastViewMode: 'trash' }), { mode: 'trash' });
  assert.deepEqual(resolveStartupTarget({ ...base, lastViewMode: 'unfiled' }), { mode: 'unfiled' });
});

test('disabled restoration and obsolete startup values use a safe fallback', () => {
  assert.deepEqual(resolveStartupTarget({
    ...base,
    startupView: 'daily',
    restoreLastSession: false
  }), { mode: 'all' });
  assert.equal(normalizeStartupView('daily'), 'all');
});
