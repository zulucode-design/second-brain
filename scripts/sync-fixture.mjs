#!/usr/bin/env node
// Deterministic 5,000-note sync fixture for the issue #97 interruption protocol.
//
//   node scripts/sync-fixture.mjs generate <vault>   write the fixture into a disposable vault
//   node scripts/sync-fixture.mjs check <vault>      print one timestamped JSON observation
//
// Both machines run the same script, so the checker never trusts anything but the files on
// disk: it re-derives every expected note and compares content, not just counts. `check`
// exits 0 only when the vault holds exactly the fixture with no duplicates or conflict copies,
// 1 while it is incomplete or wrong, and 2 on usage errors.

import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { hostname } from 'node:os';
import { join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

export const NOTE_COUNT = 5000;
export const SENTINEL_INDEX = NOTE_COUNT - 1;
export const FIXTURE_FOLDER = 'Sync Fixture 97';
const CATEGORIES = ['Projects', 'Areas', 'Resources', 'Archives'];
const ID_PREFIX = '00000097-0000-4000-8000-';
const TIMESTAMP = '2026-09-14T00:00:00+00:00';

export function fixtureNote(index) {
  const number = String(index).padStart(4, '0');
  const category = CATEGORIES[index % CATEGORIES.length];
  const sentinel = index === SENTINEL_INDEX;
  const title = sentinel ? 'Fixture sentinel' : `Fixture ${number}`;
  // The sentinel is the last note written and carries a larger body, so a transfer that stops
  // early or truncates a file is visible in its hash rather than only in the count.
  const body = sentinel
    ? Array.from({ length: 200 }, (_, line) => `Sentinel line ${line}: ${'x'.repeat(line % 61)}`).join('\n')
    : `Body of fixture note ${number}.`;
  const content =
    `---\nid: "${ID_PREFIX}${String(index).padStart(12, '0')}"\ntitle: "${title}"\ntags: []\n` +
    `pinned: false\ncreated: ${TIMESTAMP}\nmodified: ${TIMESTAMP}\ncategory: ${category}\n---\n` +
    `${body}\n`;
  return { path: join(category, FIXTURE_FOLDER, `${title}.md`), content };
}

export function sha256(text) {
  return createHash('sha256').update(text).digest('hex');
}

export function generate(vault) {
  if (!existsSync(join(vault, '.helixnotes', 'vault_id'))) {
    throw new Error(`not a vault (missing .helixnotes/vault_id): ${vault}`);
  }
  for (const category of CATEGORIES) {
    if (existsSync(join(vault, category, FIXTURE_FOLDER))) {
      throw new Error(`fixture already present in ${category}; use a fresh disposable vault copy`);
    }
  }
  for (let index = 0; index < NOTE_COUNT; index += 1) {
    const note = fixtureNote(index);
    mkdirSync(join(vault, note.path, '..'), { recursive: true });
    writeFileSync(join(vault, note.path), note.content, { flag: 'wx' });
  }
  return { notes: NOTE_COUNT, sentinelSha256: sha256(fixtureNote(SENTINEL_INDEX).content) };
}

function walk(root, visit) {
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) walk(path, visit);
    else if (entry.isFile()) visit(path);
  }
}

export function check(vault) {
  const expected = new Map();
  for (let index = 0; index < NOTE_COUNT; index += 1) {
    const note = fixtureNote(index);
    expected.set(note.path.split(sep).join('/'), note.content);
  }

  let matching = 0;
  let wrongContent = 0;
  let sentinelPresent = false;
  const conflictCopies = [];
  const strays = [];
  const idCounts = new Map();

  // The whole vault, including .helixnotes/: a fixture id surfacing in trash, the holding
  // area, or a second folder is a duplicate or resurrection, not a harmless extra file.
  walk(vault, (path) => {
    const rel = relative(vault, path).split(sep).join('/');
    if (rel.startsWith('.stfolder') || rel.startsWith('.stversions')) return;
    if (!rel.endsWith('.md')) return;
    const content = readFileSync(path, 'utf8');
    const id = /^id: "([^"]+)"$/m.exec(content)?.[1];
    if (id?.startsWith(ID_PREFIX)) idCounts.set(id, (idCounts.get(id) ?? 0) + 1);
    // Only fixture conflicts count. A reused disposable vault can already hold resolved,
    // unrelated conflict copies archived in trash, and those say nothing about this run. A
    // conflict copy is recognised by its id or, if truncated, by the note it was copied from.
    const conflictOf = rel.replace(/\.sync-conflict-[^.]*(?=\.md$)/, '');
    if (conflictOf !== rel && (id?.startsWith(ID_PREFIX) || expected.has(conflictOf))) {
      conflictCopies.push(rel);
    }
    // Judge fixture paths by content before identity: a truncated transfer can cut off the
    // id line itself, and that is wrong content, not a missing note.
    const want = expected.get(rel);
    if (want === undefined) {
      if (id?.startsWith(ID_PREFIX) && !rel.includes('.sync-conflict-')) strays.push(rel);
    } else if (content === want) {
      matching += 1;
      if (rel.endsWith('Fixture sentinel.md')) sentinelPresent = true;
    } else {
      wrongContent += 1;
    }
  });

  const duplicateIds = [...idCounts.values()].filter((count) => count > 1).length;
  const complete =
    matching === NOTE_COUNT &&
    sentinelPresent &&
    wrongContent === 0 &&
    duplicateIds === 0 &&
    conflictCopies.length === 0 &&
    strays.length === 0;

  return {
    at: new Date().toISOString(),
    host: hostname(),
    vault,
    expected: NOTE_COUNT,
    matching,
    missing: NOTE_COUNT - matching - wrongContent,
    wrongContent,
    sentinelPresent,
    duplicateIds,
    conflictCopies: conflictCopies.slice(0, 20),
    conflictCopyCount: conflictCopies.length,
    strays: strays.slice(0, 20),
    strayCount: strays.length,
    complete
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [command, vault] = process.argv.slice(2);
  try {
    if (command === 'generate' && vault) {
      console.log(JSON.stringify({ at: new Date().toISOString(), host: hostname(), ...generate(vault) }));
    } else if (command === 'check' && vault) {
      const observation = check(vault);
      console.log(JSON.stringify(observation));
      process.exit(observation.complete ? 0 : 1);
    } else {
      console.error('usage: node scripts/sync-fixture.mjs generate|check <vault>');
      process.exit(2);
    }
  } catch (error) {
    console.error(error.message);
    process.exit(2);
  }
}
