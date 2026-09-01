import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/note-creation.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'note-creation.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { destinationForCategory, suggestedNotebookForCreation } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

test('global creation never infers a folder outside the active notebook view', () => {
  assert.equal(suggestedNotebookForCreation('all', 'Projects/Open note folder'), null);
  assert.equal(suggestedNotebookForCreation('tag', 'Areas/Health'), null);
  assert.equal(suggestedNotebookForCreation('notebook', null), null);
});

test('a valid active subfolder is only a suggestion inside its own category', () => {
  const suggested = suggestedNotebookForCreation('notebook', 'Projects/Launch');
  assert.equal(suggested, 'Projects/Launch');
  assert.equal(destinationForCategory('Projects', suggested), 'Projects/Launch');
  assert.equal(destinationForCategory('Archives', suggested), 'Archives');
});

test('category confirmation always produces one of the four PARA destinations', () => {
  assert.deepEqual(
    ['Projects', 'Areas', 'Resources', 'Archives'].map((category) =>
      destinationForCategory(category, null)
    ),
    ['Projects', 'Areas', 'Resources', 'Archives']
  );
});
