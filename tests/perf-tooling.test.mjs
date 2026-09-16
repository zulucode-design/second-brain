import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const fixture = await import(new URL('../scripts/perf-fixture.mjs', import.meta.url));
const { summarise, stats } = await import(new URL('../scripts/perf-summary.mjs', import.meta.url));

test('the fixture holds 10,000 linked notes and one 5,000-word note', (t) => {
  const vault = mkdtempSync(join(tmpdir(), 'perf-fixture-'));
  t.after(() => rmSync(vault, { recursive: true, force: true }));
  mkdirSync(join(vault, '.helixnotes'));
  writeFileSync(join(vault, '.helixnotes', 'vault_id'), 'test');
  assert.deepEqual(fixture.generate(vault), { notes: 10000, explicitLinks: 19998, longNoteWords: 5000 });
  const long = readFileSync(join(vault, fixture.longNote().path), 'utf8');
  assert.equal(long.split('\n---\n')[1].trim().split(/\s+/).length, 5000);
  assert.match(readFileSync(join(vault, fixture.fixtureNote(9998).path), 'utf8'), /\[\[Perf 00000\]\] \[\[Perf 00036\]\]/);
  assert.throws(() => fixture.generate(vault), /already present/);
});

test('stats use the nearest-rank p95 and the true median', () => {
  assert.deepEqual(stats([5, 1, 4, 2, 3]), { count: 5, median: 3, p95: 5, max: 5 });
  assert.equal(stats(Array.from({ length: 200 }, (_, i) => i + 1)).p95, 190);
});

test('startup pairs each launch with the next ready mark and budgets decide the verdict', () => {
  const rows = summarise([
    { kind: 'launch', epochMs: 1000 },
    { kind: 'startup-ready', epochMs: 3500, sidebar: true, noteList: true, editorEditable: true },
    { kind: 'launch', epochMs: 10000 },
    { kind: 'startup-ready', epochMs: 15500, sidebar: true, noteList: true, editorEditable: true },
    { kind: 'launch', epochMs: 20000 },
    { kind: 'startup-ready', epochMs: 21000, sidebar: true, noteList: true, editorEditable: false },
    { kind: 'editor-key', ms: 12 },
    { kind: 'semantic-backend', exactScanMs: 90 },
  ]);
  const byKind = Object.fromEntries(rows.map((row) => [row.kind, row]));
  assert.deepEqual(byKind.startup.samples, [2500, 5500, Infinity], 'a ready mark without an editable editor is not ready');
  assert.equal(byKind.startup.pass, false, 'one run over 5 s fails even with a passing median');
  assert.equal(byKind['editor-key'].pass, true);
  assert.equal(byKind['exact-scan'].pass, true);
  assert.equal(byKind['graph-render'].missing, true);
});
