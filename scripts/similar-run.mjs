#!/usr/bin/env node
// Runs the capture-time similarity check (#9) in a real app build and records what it did.
//
//   node scripts/similar-run.mjs --app <binary> [--root <dir>] [--ssh sb-windows]
//
// Same setup as ask-run.mjs: a fresh run root, tauri-driver, and the desktop's Ollama tunnelled
// in. The vault holds scripts/similarity-fixture.json's notes. Each fixture capture is filed
// through the command the overlay uses (WebDriver cannot press a global hotkey), and the card
// it raises in the main window is read back. Then Append, Dismiss, refused Appends on a note
// edited and a note moved since the check, and a capture with the backend gone are each tried
// once.
// On Linux, two more duplicates are captured with the main window minimized, and the
// notifications the app sends are read off the session bus with dbus-monitor. A click on one
// cannot be scripted, so that stays a hand check.

import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFileSync, existsSync, mkdirSync, readdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { basename, join, relative } from 'node:path';
import { invoke, options, runMain, sleep, waitForIndex, withApp } from './ask-run.mjs';

const fixture = JSON.parse(readFileSync(new URL('./similarity-fixture.json', import.meta.url), 'utf8'));
const CARD_TIMEOUT = 60_000;
const TIMESTAMP = '2026-10-05T00:00:00+00:00';

const noteId = (index) => `00000009-0000-4000-8000-${String(index + 1).padStart(12, '0')}`;

function fill(vault) {
  fixture.notes.forEach(({ title, category, body }, index) => {
    mkdirSync(join(vault, category), { recursive: true });
    const id = noteId(index);
    writeFileSync(
      join(vault, category, `${title}.md`),
      `---\nid: "${id}"\ntitle: "${title}"\ntags: []\npinned: false\ncreated: ${TIMESTAMP}\n` +
        `modified: ${TIMESTAMP}\ncategory: ${category}\n---\n${body}\n`,
      { flag: 'wx' },
    );
  });
}

// A save rewrites the frontmatter (its modified time), so text is compared below it.
const body = (raw) => raw.replace(/^---\n[\s\S]*?\n---\n/, '');

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
      failure: card.querySelector('.similar-error')?.innerText.trim() ?? null,
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
  if (entry.failed) throw new Error(`capture failed: ${entry.failed}`);
  return entry;
}

// The app's Notify calls on the session bus, as dbus-monitor prints them: each call's
// replaces_id, summary, and body. Other apps' notifications are left out, and so is GNOME
// relaying each call on to another bus client.
function notifyCalls(output) {
  return output
    .split(/^method call /m)
    .slice(1)
    .filter((call) => call.split('\n')[0].includes('destination=org.freedesktop.Notifications '))
    .map((call) => [...call.matchAll(/^\s+(string|uint32) "?(.*?)"?$/gm)].map((match) => match[2]))
    .filter((values) => values[0] === 'Second Brain')
    .map(([, replacesId, , summary, body]) => ({ replacesId: Number(replacesId), summary, body }));
}

const trashed = (vault, path) =>
  readdirSync(join(vault, '.helixnotes', 'trash'), { recursive: true }).some((name) =>
    String(name).endsWith(basename(path)),
  );

async function main() {
  const settings = options(process.argv.slice(2), 'sb-similar-run');
  await withApp({ ...settings, fill }, async (browser, { vault, tunnel }) => {
    let busOutput = '';
    const bus = process.platform === 'linux'
      ? spawn('dbus-monitor', ['--session', "type='method_call',interface='org.freedesktop.Notifications',member='Notify'"])
      : null;
    bus?.stdout.on('data', (chunk) => (busOutput += chunk));
    process.on('exit', () => bus?.kill());
    const index = await waitForIndex(browser, fixture.notes.length);
    const results = [];
    for (const item of fixture.captures) {
      const started = Date.now();
      const entry = await capture(browser, item.text);
      // The overlay closes once this command returns, so this is what the user waits for.
      const captureMs = Date.now() - started;
      // The backend's own check sends the card; this asks the same question directly, so the
      // run knows whether a card is due rather than waiting out a timeout.
      const expected = await invoke(browser, 'check_similar_notes', { path: entry.path });
      if (expected?.failed) throw new Error(`check failed: ${expected.failed}`);
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
        captureMs,
        // From the capture to the card, or to the end of the 5-second wait when none is due.
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
      const history = join(vault, '.helixnotes', 'history', noteId(fixture.notes.findIndex((note) => note.title === appended.duplicateOf)));
      const before = readFileSync(target, 'utf8');
      // A snapshot from a moment ago, as if the note had just been saved: a save's own snapshot
      // is skipped within 5 minutes of the last, so only the append's forced one keeps `before`.
      mkdirSync(history, { recursive: true });
      writeFileSync(join(history, `${new Date().toISOString().slice(0, 19).replaceAll(':', '-')}.md`), 'an earlier save\n');
      await press(browser, appended.title, 'Append', appended.duplicateOf);
      await waitForCard(browser, appended.title, (card) => card === null);
      const after = readFileSync(target, 'utf8');
      actions.append = {
        capture: appended.capture,
        target: appended.duplicateOf,
        keptOriginal: body(after).trimStart().startsWith(body(before).trim()),
        marked: after.includes('*Added from capture,'),
        hasCapture: after.includes(appended.title),
        captureGone: !existsSync(appended.path),
        captureInTrash: trashed(vault, appended.path),
        // The note's history holds its text from before the append, so the append can be undone.
        undoable: existsSync(history) && readdirSync(history).some((name) => readFileSync(join(history, name), 'utf8') === before),
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
      const card = await waitForCard(browser, changed.title, (current) => Boolean(current?.failure));
      actions.changedNote = {
        capture: changed.capture,
        failure: card.failure,
        unchanged: hash(changed.path) === hashes[0] && hash(target) === hashes[1],
      };
    }

    // The main window minimized: each duplicate also raises a notification that names no note,
    // and the second replaces the first. Every capture above ran with the window focused.
    if (bus) {
      await sleep(2_000);
      const whileFocused = notifyCalls(busOutput).length;
      await invoke(browser, 'plugin:window|minimize', { label: 'main' });
      await sleep(1_000);
      for (const item of fixture.captures.filter((capture) => capture.duplicateOf).slice(3, 5)) {
        const entry = await capture(browser, `${item.text} (unfocused)`);
        await waitForCard(browser, entry.meta.title);
        await sleep(2_000);
      }
      const calls = notifyCalls(busOutput).slice(whileFocused);
      actions.notification = {
        whileFocused,
        calls,
        replaced: calls.length === 2 && calls[0].replacesId === 0 && calls[1].replacesId > 0,
      };
      bus.kill();
    }

    // A note moved since the check: the append is refused and nothing is written.
    const moved = carded[5];
    if (moved) {
      const target = notePath(moved.duplicateOf);
      const movedTo = `${target.slice(0, -3)} (moved).md`;
      renameSync(target, movedTo);
      const hashes = [hash(moved.path), hash(movedTo)];
      await press(browser, moved.title, 'Append', moved.duplicateOf);
      const card = await waitForCard(browser, moved.title, (current) => Boolean(current?.failure));
      actions.movedNote = {
        capture: moved.capture,
        failure: card.failure,
        unchanged: hash(moved.path) === hashes[0] && hash(movedTo) === hashes[1],
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
      app: basename(settings.app),
      index,
      summary: {
        captures: results.length,
        duplicatesFound: results.filter((result) => result.duplicateOf && !result.missed).length,
        duplicates: results.filter((result) => result.duplicateOf).length,
        wrongCards: results.filter((result) => result.wrong.length > 0).length,
      },
      // Vault-relative, so the trace names no home directory when it is committed as evidence.
      results: results.map((result) => ({ ...result, path: relative(vault, result.path) })),
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
