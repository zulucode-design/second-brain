import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';
import { Editor } from '@tiptap/core';
import StarterKit from '@tiptap/starter-kit';

async function importTypeScript(relativePath) {
  const source = await readFile(new URL(relativePath, import.meta.url), 'utf8');
  const { code } = await transformWithEsbuild(source, relativePath, { loader: 'ts', format: 'esm', target: 'esnext' });
  return import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
}

// Bare specifiers do not resolve from a data: URL, so the helper's TipTap types are dropped
// by esbuild and only its runtime code is loaded.
const { isEditUpdate, loadContent } = await importTypeScript('../src/lib/utils/editor-updates.ts');
const { SaveCoordinator } = await importTypeScript('../src/lib/utils/save-coordinator.ts');

const doc = (text) => ({ type: 'doc', content: [{ type: 'paragraph', content: [{ type: 'text', text }] }] });

// A headless TipTap editor wired the way Editor.svelte wires it: updates that are edits mark
// the save coordinator dirty, and a flush persists what the editor holds.
function wiredEditor(text) {
  const writes = [];
  let loading = false;
  const editor = new Editor({ extensions: [StarterKit], content: doc(text) });
  const coordinator = new SaveCoordinator({
    delayMs: 500,
    capture: () => ({ path: '/vault/note.md', body: editor.getText() }),
    persist: async (snapshot) => { writes.push(snapshot.body); },
    onDirtyChange: () => {},
    setTimer: () => 1,
    clearTimer: () => {},
  });
  coordinator.setDocument('/vault/note.md');
  editor.on('update', (event) => { if (isEditUpdate(event, loading)) coordinator.markDirty(); });
  const load = (next) => { loading = true; loadContent(editor, doc(next)); loading = false; };
  return { editor, coordinator, writes, load };
}

test('closing or toggling View Mode does not write an unedited note (#196)', async () => {
  const { editor, coordinator, writes } = wiredEditor('unchanged');
  editor.setEditable(false); // TipTap's default emits an update with no document change
  editor.setEditable(true);
  assert.equal(coordinator.isDirty(), false);
  await coordinator.flush();
  assert.deepEqual(writes, []);
});

test('the first edit after loading identical content is saved', async () => {
  const { editor, coordinator, writes, load } = wiredEditor('same');
  load('same'); // TipTap emits nothing for an identical document
  editor.commands.insertContent({ type: 'text', text: ' pasted' });
  assert.equal(coordinator.isDirty(), true);
  await coordinator.flush();
  assert.deepEqual(writes, ['same pasted']);
});

test('loading different content is not an edit, and the edit after it is', async () => {
  const { editor, coordinator, writes, load } = wiredEditor('first note');
  load('second note');
  assert.equal(coordinator.isDirty(), false);
  editor.commands.insertContent({ type: 'text', text: '!' });
  await coordinator.flush();
  assert.deepEqual(writes, ['second note!']);
});

test('Editor.svelte routes every update and content swap through the helpers', async () => {
  const source = await readFile(new URL('../src/lib/components/Editor.svelte', import.meta.url), 'utf8');
  assert.match(source, /onUpdate: \(event\) => \{\s*if \(!isEditUpdate\(event, isLoadingNote\)\) return;/);
  assert.doesNotMatch(source, /ignoreNextUpdate/);
  // Loads go through loadContent; history restores and AI replacements are edits and still emit.
  assert.equal([...source.matchAll(/loadContent\(editor, /g)].length, 2);
  for (const [, args] of source.matchAll(/\.setEditable\(([^;]*)\);/g)) assert.match(args, /, false$/);
});
