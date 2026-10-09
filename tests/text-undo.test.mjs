import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(new URL('../src/lib/utils/text-undo.ts', import.meta.url), 'utf8');
const { code } = await transformWithEsbuild(source, 'text-undo.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { undoCommand } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);

test('Ctrl+Z undoes; Ctrl+Shift+Z and Ctrl+Y redo (#213)', () => {
  assert.equal(undoCommand({ key: 'z', ctrlKey: true }), 'undo');
  assert.equal(undoCommand({ key: 'Z', ctrlKey: true, shiftKey: true }), 'redo');
  assert.equal(undoCommand({ key: 'y', ctrlKey: true }), 'redo');
  assert.equal(undoCommand({ key: 'z', metaKey: true }), 'undo');
});

test('other keys and combinations are left alone', () => {
  assert.equal(undoCommand({ key: 'z' }), null);
  assert.equal(undoCommand({ key: 'z', ctrlKey: true, altKey: true }), null);
  assert.equal(undoCommand({ key: 'y', ctrlKey: true, shiftKey: true }), null);
  assert.equal(undoCommand({ key: 'a', ctrlKey: true }), null);
});
