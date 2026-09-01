import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/note-row-policy.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'note-row-policy.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { noteRowPolicy } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

test('holding rows expose preview and filing without normal note actions', () => {
  assert.deepEqual(noteRowPolicy('unfiled'), {
    holdingPreview: true,
    fileUnder: true,
    rename: false,
    drag: false,
    contextMenu: false,
    quickAccess: false
  });
});

test('ordinary note rows retain their normal actions', () => {
  for (const mode of ['all', 'notebook', 'tag', 'quickaccess', 'tasks', 'trash']) {
    const policy = noteRowPolicy(mode);
    assert.equal(policy.holdingPreview, false, mode);
    assert.equal(policy.fileUnder, false, mode);
    assert.equal(policy.rename, true, mode);
    assert.equal(policy.drag, true, mode);
    assert.equal(policy.contextMenu, true, mode);
    assert.equal(policy.quickAccess, true, mode);
  }
});
