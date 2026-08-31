import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/para-ui-policy.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'para-ui-policy.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const policy = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

test('the four PARA roots can contain folders but cannot be mutated or reordered', () => {
  for (const root of ['Projects', 'Areas', 'Resources', 'Archives']) {
    assert.deepEqual(policy.notebookUiPolicy(root), {
      createChild: true,
      rename: false,
      delete: false,
      move: false,
      reorder: false,
      setIcon: true
    });
  }
});

test('ordinary folders stay editable only within PARA destinations', () => {
  assert.deepEqual(policy.notebookUiPolicy('Projects/Launch'), {
    createChild: true,
    rename: true,
    delete: true,
    move: true,
    reorder: true,
    setIcon: true
  });
  assert.equal(policy.canCreateNotebookUnder(null), false);
  assert.equal(policy.canCreateNotebookUnder('Inbox'), false);
  assert.equal(policy.canCreateNotebookUnder('Areas/Health'), true);
  assert.equal(policy.canMoveNotebookTo('Inbox', 'Resources'), true);
  assert.equal(policy.canMoveNotebookTo('Projects', 'Areas'), false);
  assert.equal(policy.canMoveNotebookTo('Projects/Launch', 'Inbox'), false);
  assert.equal(policy.canReorderNotebookBeside('Projects/Launch', 'Areas'), false);
  assert.equal(policy.canReorderNotebookBeside('Projects/Launch', 'Areas/Health'), true);
});

test('Windows separators follow the same category policy', () => {
  assert.equal(policy.isParaCategoryRoot('Projects'), true);
  assert.equal(policy.isInsideParaCategory('Projects\\Launch'), true);
  assert.equal(policy.canCreateNotebookUnder('Areas\\Health'), true);
});
