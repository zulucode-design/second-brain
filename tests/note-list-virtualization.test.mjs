import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/note-list-virtualization.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'note-list-virtualization.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { noteListWindow } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

test('all 200 variable-height holding rows stay reachable', () => {
  for (const scrollTop of [0, 4_000, 12_000]) {
    assert.deepEqual(noteListWindow({
      viewMode: 'unfiled',
      itemCount: 200,
      itemHeight: 62,
      scrollTop,
      containerHeight: 600
    }), {
      startIndex: 0,
      endIndex: 200,
      topPad: 0,
      bottomPad: 0
    });
  }
});

test('fixed-height note views keep their bounded virtual window', () => {
  const window = noteListWindow({
    viewMode: 'all',
    itemCount: 200,
    itemHeight: 62,
    scrollTop: 6_200,
    containerHeight: 620
  });
  assert.ok(window.startIndex > 0);
  assert.ok(window.endIndex < 200);
  assert.equal(window.topPad, window.startIndex * 62);
  assert.equal(window.bottomPad, (200 - window.endIndex) * 62);
});
