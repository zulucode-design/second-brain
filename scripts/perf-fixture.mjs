#!/usr/bin/env node
// Deterministic 10,000-note performance fixture for the issue #88 budgets.
//
//   node scripts/perf-fixture.mjs generate <vault>   write the fixture into a disposable vault
//
// 9,999 short notes each link to two others by title, so the graph has a known explicit-link
// count. Bodies rotate through a few topics so semantic search has something to rank. The
// 10,000th note holds exactly 5,000 words for the editor budget.

import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const NOTE_COUNT = 10000;
const LINKED_NOTES = NOTE_COUNT - 1;
export const LONG_NOTE_WORDS = 5000;
export const FIXTURE_FOLDER = 'Perf Fixture 88';
export const LONG_NOTE_TITLE = 'Perf long note';
const CATEGORIES = ['Projects', 'Areas', 'Resources', 'Archives'];
const TIMESTAMP = '2026-09-16T00:00:00+00:00';
const TOPICS = [
  'Sourdough bread needs a lively starter, a long cold proof, and a very hot oven.',
  'Tomato seedlings want bright light, steady watering, and support once they grow tall.',
  'Rust ownership moves values between bindings and borrowing avoids needless copies.',
  'A marathon plan builds weekly mileage slowly and keeps most runs at an easy pace.',
  'Budget reviews compare planned spending with actual receipts at the end of each month.',
  'Jazz improvisation borrows phrases from the melody and resolves tension on strong beats.',
  'Bicycle maintenance means cleaning the chain, checking tyre pressure, and truing wheels.',
  'Coastal birds migrate along the shoreline each autumn and rest in the salt marshes.',
];

const title = (index) => `Perf ${String(index).padStart(5, '0')}`;

function note(category, name, id, body) {
  const content =
    `---\nid: "00000088-0000-4000-8000-${id}"\ntitle: "${name}"\ntags: []\n` +
    `pinned: false\ncreated: ${TIMESTAMP}\nmodified: ${TIMESTAMP}\ncategory: ${category}\n---\n${body}\n`;
  return { path: join(category, FIXTURE_FOLDER, `${name}.md`), content };
}

export function fixtureNote(index) {
  const links = [(index + 1) % LINKED_NOTES, (index + 37) % LINKED_NOTES].map((i) => `[[${title(i)}]]`);
  const body = `${TOPICS[index % TOPICS.length]}\n\nRelated: ${links.join(' ')}`;
  return note(CATEGORIES[index % CATEGORIES.length], title(index), String(index).padStart(12, '0'), body);
}

export function longNote() {
  const words = [];
  for (let sentence = 0; words.length < LONG_NOTE_WORDS; sentence += 1) {
    words.push(...TOPICS[sentence % TOPICS.length].split(' '));
  }
  const paragraphs = [];
  for (let start = 0; start < LONG_NOTE_WORDS; start += 100) {
    paragraphs.push(words.slice(start, Math.min(start + 100, LONG_NOTE_WORDS)).join(' '));
  }
  return note('Resources', LONG_NOTE_TITLE, '999999999999', paragraphs.join('\n\n'));
}

export function generate(vault) {
  if (!existsSync(join(vault, '.helixnotes', 'vault_id'))) {
    throw new Error(`not a vault (missing .helixnotes/vault_id): ${vault}`);
  }
  if (CATEGORIES.some((category) => existsSync(join(vault, category, FIXTURE_FOLDER)))) {
    throw new Error('fixture already present; use a fresh disposable vault');
  }
  const notes = Array.from({ length: LINKED_NOTES }, (_, index) => fixtureNote(index));
  notes.push(longNote());
  for (const { path, content } of notes) {
    mkdirSync(join(vault, path, '..'), { recursive: true });
    writeFileSync(join(vault, path), content, { flag: 'wx' });
  }
  return { notes: notes.length, explicitLinks: LINKED_NOTES * 2, longNoteWords: LONG_NOTE_WORDS };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [command, vault] = process.argv.slice(2);
  if (command !== 'generate' || !vault) {
    console.error('usage: perf-fixture.mjs generate <vault>');
    process.exit(2);
  }
  console.log(JSON.stringify(generate(vault)));
}
