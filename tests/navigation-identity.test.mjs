import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(new URL('../src/lib/utils/navigation.ts', import.meta.url), 'utf8');
const { code } = await transformWithEsbuild(source, 'navigation.ts', { loader: 'ts', format: 'esm', target: 'esnext' });
const { isExpectedNote } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);

test('a note read for navigation must be the expected one when an id is given', () => {
  assert.equal(isExpectedNote({ meta: { id: 'cited' } }, 'cited'), true);
  assert.equal(isExpectedNote({ meta: { id: 'replacement' } }, 'cited'), false);
  assert.equal(isExpectedNote({ meta: { id: 'anything' } }, null), true);
  assert.equal(isExpectedNote({ meta: { id: 'anything' } }, undefined), true);
});

test('navigation checks identity on the note it reads after waiting on a save, and on the open note', async () => {
  const layout = await readFile(new URL('../src/lib/components/AppLayout.svelte', import.meta.url), 'utf8');
  const body = layout.slice(layout.indexOf('async function navigateToPathResult('), layout.indexOf('async function navigateToPath('));
  assert.match(body, /if \(\$activeNote && !isExpectedNote\(\$activeNote, expectedId\)\) return 'not-found';/);
  // The check follows the read that happens after the queued save, so a note swapped in
  // during that wait is caught before it is committed to the editor.
  const read = body.indexOf('await readNote(path)');
  const check = body.indexOf("if (!isExpectedNote(content, expectedId)) return 'not-found';");
  const commit = body.indexOf('commitNote(path, content');
  assert.ok(read > 0 && check > read && commit > check);
  assert.match(layout, /<SearchPanel onOpenResult=\{\(path, noteId\) => navigateToPathResult\(path, undefined, false, noteId\)\} \/>/);
});
