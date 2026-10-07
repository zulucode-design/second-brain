import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(new URL('../src/lib/utils/similar.ts', import.meta.url), 'utf8');
const { code } = await transformWithEsbuild(source, 'similar.ts', { loader: 'ts', format: 'esm', target: 'esnext' });
const { withCard, forVault, similarCoverage, createLeaveWatcher, appendWhileHeld } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

const check = (capture, matches = 1, total = matches) => ({
  capture: { path: capture, title: capture, revision: 'r' },
  matches: Array.from({ length: matches }, (_, index) => ({ path: `/v/m${index}.md`, title: `M${index}`, revision: 'r', excerpt: '' })),
  total,
});
const card = (id, capture, vault = '/v') => ({ id, vault, check: check(capture), busy: false, error: null });

test('a new card goes first and replaces an older check of the same note', () => {
  const cards = [card('b', '/v/two.md'), card('a', '/v/one.md')];

  assert.deepEqual(withCard(cards, card('c', '/v/three.md')).map((item) => item.id), ['c', 'b', 'a']);
  assert.deepEqual(withCard(cards, card('d', '/v/one.md')).map((item) => item.id), ['d', 'b']);
});

test('a vault switch drops every card from the vault that was left', () => {
  const cards = [card('b', '/old/x.md', '/old'), card('a', '/new/y.md', '/new')];

  assert.deepEqual(forVault(cards, '/new').map((item) => item.id), ['a']);
  assert.deepEqual(forVault(cards, null), []);
});

test('the coverage line appears only when more notes passed than the card shows', () => {
  assert.equal(similarCoverage(check('/v/c.md', 3, 5)), 'Showing 3 of 5 similar notes');
  assert.equal(similarCoverage(check('/v/c.md', 2, 2)), null);
});

test('a note made in the app is checked once, when the user first leaves it', () => {
  const watcher = createLeaveWatcher();
  watcher.created('new');

  assert.equal(watcher.opened({ id: 'new', path: '/v/Untitled.md' }), null);
  // Retitling renames the file: still the same note, so not a leave.
  assert.equal(watcher.opened({ id: 'new', path: '/v/Cedar.md' }), null);
  assert.equal(watcher.opened({ id: 'old', path: '/v/Old.md' }), '/v/Cedar.md');
  assert.equal(watcher.opened({ id: 'new', path: '/v/Cedar.md' }), null);
  assert.equal(watcher.opened(null), null, 'only the first leave checks it');
});

test('closing a new note counts as leaving it; older notes are never checked', () => {
  const watcher = createLeaveWatcher();
  watcher.opened({ id: 'old', path: '/v/Old.md' });
  assert.equal(watcher.opened({ id: 'other', path: '/v/Other.md' }), null);

  watcher.created('new');
  watcher.opened({ id: 'new', path: '/v/New.md' });
  assert.equal(watcher.opened(null), '/v/New.md');
});

test('an append holds the open note read-only from its save until the result shows', async () => {
  const runAppend = async ({ held = true, saved = true, outcome = 'appended', fails = false } = {}) => {
    const steps = [];
    let readOnly = false;
    await appendWhileHeld({
      held,
      hold: async () => {
        readOnly = true;
        steps.push('hold');
        return () => {
          readOnly = false;
          steps.push('release');
        };
      },
      save: async () => (steps.push(`save read-only=${readOnly}`), saved),
      append: async () => {
        steps.push(`append read-only=${readOnly}`);
        if (fails) throw new Error('backend gone');
        return outcome;
      },
      settle: (result) => steps.push(`settle ${result} read-only=${readOnly}`),
    }).catch(() => steps.push('threw'));
    return steps;
  };

  assert.deepEqual(await runAppend(), ['hold', 'save read-only=true', 'append read-only=true', 'settle appended read-only=true', 'release']);
  assert.deepEqual(await runAppend({ saved: false }), ['hold', 'save read-only=true', 'release'], 'an unsaved note stops the append');
  assert.deepEqual(await runAppend({ outcome: null }), ['hold', 'save read-only=true', 'append read-only=true', 'release'], 'nothing written, nothing to show');
  assert.deepEqual(await runAppend({ fails: true }), ['hold', 'save read-only=true', 'append read-only=true', 'release', 'threw']);
  assert.deepEqual(await runAppend({ held: false, outcome: 'capture-kept' }), ['append read-only=false', 'settle capture-kept read-only=false'], 'a note not open is not held');
});
