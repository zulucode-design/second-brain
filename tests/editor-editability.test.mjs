import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

// #196: TipTap's setEditable(editable, emitUpdate = true) emits `update` with an empty
// transaction. Closing the app or toggling View Mode then marked the note dirty and rewrote it.
test('changing editability never marks the open note dirty', async () => {
  const editor = await readFile(new URL('../src/lib/components/Editor.svelte', import.meta.url), 'utf8');
  const calls = [...editor.matchAll(/\.setEditable\(([^;]*)\);/g)].map((match) => match[1]);
  assert.ok(calls.length >= 3, 'the editor still toggles editability');
  for (const args of calls) assert.match(args, /, false$/, `setEditable(${args}) emits an update`);
  assert.match(editor, /onUpdate: \(\{ transaction \}\) => \{\s*(\/\/[^\n]*\n\s*)*if \(!transaction\.docChanged\) return;/);
});
