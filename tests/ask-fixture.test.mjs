import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const fixture = await import(new URL('../scripts/ask-fixture.mjs', import.meta.url));

test('writes the Ask fixture into a vault once, with every behaviour it promises', () => {
  const vault = mkdtempSync(join(tmpdir(), 'ask-fixture-'));
  try {
    mkdirSync(join(vault, '.helixnotes'));
    writeFileSync(join(vault, '.helixnotes', 'vault_id'), 'test');

    const result = fixture.generate(vault);

    assert.equal(result.notes, 40);
    const read = (category, title) =>
      readFileSync(join(vault, category, fixture.FIXTURE_FOLDER, `${title}.md`), 'utf8');
    const article = read('Resources', 'How the printing press changed Europe');
    assert.ok(article.split(/\s+/).length >= 5000);
    assert.match(read('Resources', 'Cold brew coffee guide'), /ignore all previous instructions/);
    assert.match(read('Resources', 'Receta de arepas'), /harina de maíz/);
    assert.throws(() => fixture.generate(vault), /fixture already present/);
  } finally {
    rmSync(vault, { recursive: true, force: true });
  }
});

test('every question cites notes that exist, and one has nothing to cite', () => {
  const titles = new Set(fixture.notes().map((note) => note.path.split(/[\\/]/).pop().replace(/\.md$/, '')));
  for (const { question, cites } of fixture.QUESTIONS) {
    for (const title of cites) assert.ok(titles.has(title), `${question} cites missing note ${title}`);
  }
  assert.ok(fixture.QUESTIONS.some(({ cites }) => cites.length === 0));
});

test('refuses a folder that is not a vault', () => {
  const folder = mkdtempSync(join(tmpdir(), 'ask-fixture-'));
  try {
    assert.throws(() => fixture.generate(folder), /not a vault/);
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
});
