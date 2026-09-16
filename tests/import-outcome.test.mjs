import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/import-outcome.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'import-outcome.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { importOutcomeView } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

const counts = {
  files_converted: 2,
  links_converted: 3,
  frontmatter_normalized: 2,
  syntax_converted: 2,
  attachments_moved: 0
};

test('a clean import shows its counts and no error', () => {
  const view = importOutcomeView({ success: true, outcome: 'success', ...counts });
  assert.deepEqual(view.result, counts);
  assert.equal(view.error, null);
  assert.equal(view.refresh, true);
});

test('a partial import shows what converted alongside the reason it stopped', () => {
  const view = importOutcomeView({
    success: false,
    outcome: 'changed-incomplete',
    error: '2 file(s) could not be converted: locked/frozen-one.md: Permission denied',
    ...counts
  });
  // Both halves: the counts are not dropped just because the run also failed.
  assert.deepEqual(view.result, counts);
  assert.match(view.error, /Permission denied/);
  assert.equal(view.refresh, true, 'the vault changed, so the workspace must be refreshed');
});

test('an import that changed nothing shows only the reason, and skips the refresh', () => {
  const view = importOutcomeView({
    success: false,
    outcome: 'failure',
    error: 'Another bulk vault operation is already running'
  });
  assert.equal(view.result, null);
  assert.equal(view.error, 'Another bulk vault operation is already running');
  assert.equal(view.refresh, false, 'nothing changed, so there is nothing to refresh');
});

test('a failure with no reason still says something', () => {
  const view = importOutcomeView({ success: false, outcome: 'failure' });
  assert.equal(view.error, 'Import failed');
});
