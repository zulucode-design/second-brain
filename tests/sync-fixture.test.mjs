import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, renameSync, rmSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const { NOTE_COUNT, SENTINEL_INDEX, check, fixtureNote, generate } = await import(
  new URL('../scripts/sync-fixture.mjs', import.meta.url)
);

function vault(t) {
  const root = mkdtempSync(join(tmpdir(), 'sync-fixture-'));
  mkdirSync(join(root, '.helixnotes'));
  writeFileSync(join(root, '.helixnotes', 'vault_id'), 'test');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

test('a freshly generated fixture checks complete', (t) => {
  const root = vault(t);
  generate(root);
  const observation = check(root);
  assert.equal(observation.matching, NOTE_COUNT);
  assert.equal(observation.complete, true);
});

test('generation refuses a non-vault and a vault that already has the fixture', (t) => {
  const root = vault(t);
  assert.throws(() => generate(join(root, 'missing')), /not a vault/);
  generate(root);
  assert.throws(() => generate(root), /already present/);
});

test('a missing sentinel is incomplete even though other notes arrived', (t) => {
  const root = vault(t);
  generate(root);
  unlinkSync(join(root, fixtureNote(SENTINEL_INDEX).path));
  const observation = check(root);
  assert.equal(observation.sentinelPresent, false);
  assert.equal(observation.missing, 1);
  assert.equal(observation.complete, false);
});

test('truncated content, conflict copies, and resurrected duplicates are not convergence', (t) => {
  const root = vault(t);
  generate(root);
  const first = fixtureNote(0);
  writeFileSync(join(root, first.path), first.content.slice(0, 40));
  const second = fixtureNote(1);
  writeFileSync(join(root, second.path.replace('.md', '.sync-conflict-20260914-120000-ABCDEFG.md')), second.content);
  const third = fixtureNote(2);
  mkdirSync(join(root, '.helixnotes', 'trash'), { recursive: true });
  writeFileSync(join(root, '.helixnotes', 'trash', 'copy.md'), third.content);

  const observation = check(root);
  assert.equal(observation.wrongContent, 1);
  assert.equal(observation.conflictCopyCount, 1);
  assert.equal(observation.strayCount, 1);
  assert.equal(observation.duplicateIds, 2);
  assert.equal(observation.complete, false);
});

test('unrelated conflict copies already in the vault do not fail the fixture', (t) => {
  const root = vault(t);
  generate(root);
  mkdirSync(join(root, '.helixnotes', 'trash'), { recursive: true });
  writeFileSync(
    join(root, '.helixnotes', 'trash', 'Old note.sync-conflict-20260914-153417-256O5AE.md'),
    '---\nid: "unrelated"\ntitle: "Old note"\n---\nbody\n'
  );
  const observation = check(root);
  assert.equal(observation.conflictCopyCount, 0);
  assert.equal(observation.complete, true);
});

test('a truncated conflict copy of a fixture note still counts as a conflict', (t) => {
  const root = vault(t);
  generate(root);
  const note = fixtureNote(3);
  writeFileSync(join(root, note.path.replace('.md', '.sync-conflict-20260914-120000-ABCDEFG.md')), 'cut');
  const observation = check(root);
  assert.equal(observation.conflictCopyCount, 1);
  assert.equal(observation.complete, false);
});

test('a note moved to the wrong category folder is a stray, not a match', (t) => {
  const root = vault(t);
  generate(root);
  const note = fixtureNote(0);
  const moved = join(root, 'Archives', 'moved.md');
  renameSync(join(root, note.path), moved);
  const observation = check(root);
  assert.equal(observation.matching, NOTE_COUNT - 1);
  assert.equal(observation.strayCount, 1);
  assert.equal(observation.complete, false);
});
