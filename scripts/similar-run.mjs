#!/usr/bin/env node
// Runs the capture-time similarity check (#9) in a real app build and records what it did.
//
//   node scripts/similar-run.mjs --app <binary> [--root <dir>] [--ssh sb-windows]
//
// Same setup as ask-run.mjs: a fresh run root, tauri-driver, and the desktop's Ollama tunnelled
// in. The vault holds scripts/similarity-fixture.json's notes. Each fixture capture is filed
// through the command the overlay uses (WebDriver cannot press a global hotkey), and the card
// it raises in the main window is read back. Then Append, Dismiss, a refused Append on a note
// changed since the check, and a capture with the backend gone are each tried once.
// The notification is not covered: WebDriver keeps the main window focused, and a script
// cannot read a desktop notification.

import { createHash } from 'node:crypto';
import { appendFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { invoke, options, runMain, sleep, waitForIndex, withApp } from './ask-run.mjs';

const fixture = JSON.parse(readFileSync(new URL('./similarity-fixture.json', import.meta.url), 'utf8'));
const CARD_TIMEOUT = 60_000;
const TIMESTAMP = '2026-10-05T00:00:00+00:00';

function fill(vault) {
  fixture.notes.forEach(({ title, category, body }, index) => {
    mkdirSync(join(vault, category), { recursive: true });
    const id = `00000009-0000-4000-8000-${String(index + 1).padStart(12, '0')}`;
    writeFileSync(
      join(vault, category, `${title}.md`),
      `---\nid: "${id}"\ntitle: "${title}"\ntags: []\npinned: false\ncreated: ${TIMESTAMP}\n` +
        `modified: ${TIMESTAMP}\ncategory: ${category}\n---\n${body}\n`,
      { flag: 'wx' },
    );
  });
}

const hash = (path) => (existsSync(path) ? createHash('sha256').update(readFileSync(path)).digest('hex') : null);

// The card for the note titled `title`, as the page shows it; null when there is none.
function readCard(browser, title) {
  return browser.execute((wanted) => {
    const card = [...document.querySelectorAll('.similar-card')].find(
      (section) => section.querySelector('.similar-heading')?.innerText.includes(`“${wanted}”`),
    );
    if (!card) return null;
    return {
      matches: [...card.querySelectorAll('.similar-match')].map((match) => ({
        title: match.querySelector('.similar-title span')?.innerText.trim(),
        excerpt: match.querySelector('.similar-excerpt')?.innerText.trim(),
      })),
      coverage: card.querySelector('.similar-coverage')?.innerText.trim() ?? null,
      error: card.querySelector('.similar-error')?.innerText.trim() ?? null,
    };
  }, title);
}

async function waitForCard(browser, title, wanted = (card) => card !== null) {
  let card = null;
  await browser.waitUntil(async () => wanted((card = await readCard(browser, title))), {
    timeout: CARD_TIMEOUT,
    interval: 500,
    timeoutMsg: `the card for "${title}" did not reach the expected state`,
  });
  return card;
}

// WebKitWebDriver rejects element-click, so buttons are clicked in the page.
function press(browser, title, label, matchTitle = null) {
  return browser.execute((wanted, text, inMatch) => {
    const card = [...document.querySelectorAll('.similar-card')].find(
      (section) => section.querySelector('.similar-heading')?.innerText.includes(`“${wanted}”`),
    );
    const scope = inMatch
      ? [...card.querySelectorAll('.similar-match')].find(
          (match) => match.querySelector('.similar-title span')?.innerText.trim() === inMatch,
        )
      : card;
    [...scope.querySelectorAll('button')].find((button) => button.innerText.trim() === text).click();
  }, title, label, matchTitle);
}

async function capture(browser, text) {
  const entry = await invoke(browser, 'quick_capture_note', { category: 'Areas', text });
  if (entry.error) throw new Error(`capture failed: ${entry.error}`);
  return entry;
}

const trashed = (vault, path) =>
  readdirSync(join(vault, '.helixnotes', 'trash'), { recursive: true }).some((name) =>
    String(name).endsWith(basename(path)),
  );

async function main() {
  const settings = options(process.argv.slice(2), 'sb-similar-run');
  await withApp({ ...settings, fill }, async (browser, { vault, tunnel }) => {
    const index = await waitForIndex(browser, fixture.notes.length);
    const results = [];
    for (const item of fixture.captures) {
      const started = Date.now();
      const entry = await capture(browser, item.text);
      // The backend's own check sends the card; this asks the same question directly, so the
      // run knows whether a card is due rather than waiting out a timeout.
      const expected = await invoke(browser, 'check_similar_notes', { path: entry.path });
      if (expected?.error) throw new Error(`check failed: ${expected.error}`);
      let card = null;
      if (expected) {
        card = await waitForCard(browser, entry.meta.title);
      } else {
        await sleep(5_000);
        card = await readCard(browser, entry.meta.title);
      }
      const shown = card?.matches.map((match) => match.title) ?? [];
      results.push({
        capture: item.text,
        kind: item.duplicateOf ? 'duplicate' : item.sameTopic?.length ? 'same topic' : 'unrelated',
        duplicateOf: item.duplicateOf ?? null,
        path: entry.path,
        title: entry.meta.title,
        seconds: Math.round((Date.now() - started) / 1000),
        shown,
        coverage: card?.coverage ?? null,
        excerpts: card?.matches.map((match) => match.excerpt) ?? [],
        missed: Boolean(item.duplicateOf) && !shown.includes(item.duplicateOf),
        wrong: shown.filter((title) => title !== item.duplicateOf),
      });
      console.log(`${shown.length ? shown.join(', ') : '(no card)'}  <-  ${item.text}`);
    }

    const carded = results.filter((result) => result.duplicateOf && result.shown.includes(result.duplicateOf));
    const notePath = (title) => {
      const note = fixture.notes.find((item) => item.title === title);
      return join(vault, note.category, `${title}.md`);
    };
    const actions = {};

    // Append: the capture lands at the end of the similar note, which keeps its text, and the
    // capture moves to trash.
    const appended = carded[0];
    if (appended) {
      const target = notePath(appended.duplicateOf);
      const before = readFileSync(target, 'utf8');
      await press(browser, appended.title, 'Append', appended.duplicateOf);
      await waitForCard(browser, appended.title, (card) => card === null);
      const after = readFileSync(target, 'utf8');
      actions.append = {
        capture: appended.capture,
        target: appended.duplicateOf,
        keptOriginal: after.startsWith(before.trimEnd()),
        marked: after.includes('*Added from capture,'),
        hasCapture: after.includes(appended.title),
        captureGone: !existsSync(appended.path),
        captureInTrash: trashed(vault, appended.path),
      };
    }

    // Dismiss: both notes stay exactly as they were.
    const dismissed = carded[1];
    if (dismissed) {
      const target = notePath(dismissed.duplicateOf);
      const hashes = [hash(dismissed.path), hash(target)];
      await press(browser, dismissed.title, 'Dismiss');
      await waitForCard(browser, dismissed.title, (card) => card === null);
      actions.dismiss = {
        capture: dismissed.capture,
        unchanged: hash(dismissed.path) === hashes[0] && hash(target) === hashes[1],
      };
    }

    // A note edited since the check: the append is refused and nothing is written.
    const changed = carded[2];
    if (changed) {
      const target = notePath(changed.duplicateOf);
      appendFileSync(target, '\nEdited after the check.\n');
      const hashes = [hash(changed.path), hash(target)];
      await press(browser, changed.title, 'Append', changed.duplicateOf);
      const card = await waitForCard(browser, changed.title, (current) => Boolean(current?.error));
      actions.changedNote = {
        capture: changed.capture,
        error: card.error,
        unchanged: hash(changed.path) === hashes[0] && hash(target) === hashes[1],
      };
    }

    // The backend gone: the capture is filed as always and no card appears.
    tunnel.kill();
    await sleep(1_000);
    const offline = await capture(browser, `${fixture.captures[0].text} (while offline)`);
    await sleep(10_000);
    actions.offline = {
      saved: existsSync(offline.path),
      card: await readCard(browser, offline.meta.title),
    };

    const trace = {
      when: new Date().toISOString(),
      app: resolve(settings.app),
      index,
      summary: {
        captures: results.length,
        duplicatesFound: results.filter((result) => result.duplicateOf && !result.missed).length,
        duplicates: results.filter((result) => result.duplicateOf).length,
        wrongCards: results.filter((result) => result.wrong.length > 0).length,
      },
      results,
      actions,
    };
    const out = join(settings.root, 'similar-run.json');
    writeFileSync(out, JSON.stringify(trace, null, 2));
    console.log(JSON.stringify(trace.summary));
    console.log(JSON.stringify(actions, null, 2));
    console.log(out);
  });
}

runMain(main);
