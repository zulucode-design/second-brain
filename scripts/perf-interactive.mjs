#!/usr/bin/env node
// Drive the installed app's interactive performance probes for issue #88.
// The caller owns tauri-driver and sets SECOND_BRAIN_PERF_LOG in its environment.

import { remote } from 'webdriverio';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const [application, portText, platform, mode = 'all'] = process.argv.slice(2);
if (!application || !portText || !['fedora', 'windows'].includes(platform) || !['all', 'editor-only', 'graph-only', 'search-only'].includes(mode)) {
  console.error('usage: perf-interactive.mjs <installed-executable> <driver-port> <fedora|windows> [all|editor-only|graph-only|search-only]');
  process.exit(2);
}

const browser = await remote({
  hostname: '127.0.0.1', port: Number(portText), logLevel: 'warn', connectionRetryCount: 0,
  capabilities: { 'tauri:options': { application } },
});

async function click(selector) {
  const element = await browser.$(selector);
  await element.waitForDisplayed({ timeout: 60_000 });
  await browser.execute((target) => target.click(), element);
}

try {
  await browser.$('button[title^="Search"]').waitForDisplayed({ timeout: 60_000 });
  await browser.setWindowSize(1280, 860);

  if (mode === 'all' || mode === 'editor-only') {
    await click('button[title^="Search"]');
    await browser.execute(() => {
      const input = document.querySelector('input[placeholder="Search notes..."]');
      input.value = 'Perf long note';
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await browser.waitUntil(async () => browser.execute(() => [...document.querySelectorAll('.result-title')]
      .some((element) => element.innerText.trim() === 'Perf long note')), {
      timeout: 60_000, timeoutMsg: 'fixture note was not found',
    });
    await browser.execute(() => [...document.querySelectorAll('.result-title')]
      .find((element) => element.innerText.trim() === 'Perf long note').closest('.result-item').click());
    await browser.waitUntil(async () => browser.execute(() => document.querySelector('.editor-title input')?.value === 'Perf long note'), {
      timeout: 30_000, timeoutMsg: 'fixture note did not open',
    });
    const words = await browser.execute(() => document.querySelector('.editor-content')?.innerText.trim().split(/\s+/).length);
    if (words < 5_000) throw new Error(`fixture note has only ${words} words`);
    await browser.pause(2000);

    const editor = await browser.$('.editor-content[contenteditable="true"]');
    await browser.execute((target) => target.focus(), editor);
    const before = await browser.execute(() => document.querySelector('.editor-content[contenteditable="true"]').innerText.length);
    if (platform === 'fedora') {
      const temporaryDirectory = mkdtempSync(join(tmpdir(), 'second-brain-perf-keys-'));
      const keyboard = join(temporaryDirectory, 'perf-key-input');
      try {
        execFileSync('cc', ['-O2', join(dirname(fileURLToPath(import.meta.url)), 'perf-key-input.c'), '-o', keyboard]);
        execFileSync(keyboard, [], { timeout: 30_000 });
      } finally {
        rmSync(temporaryDirectory, { recursive: true, force: true });
      }
    } else {
      for (let index = 0; index < 200; index += 1) {
        await editor.addValue('x');
        await browser.pause(50);
      }
    }
    const after = await browser.execute(() => document.querySelector('.editor-content[contenteditable="true"]').innerText.length);
    if (after !== before + 200) throw new Error(`editor kept ${after - before} of 200 inserted keys`);
    await browser.pause(1000);
    console.log(`editor: 200 keys on ${words}-word note`);
  }

  if (mode === 'all' || mode === 'graph-only') {
    for (let run = 1; run <= 5; run += 1) {
      await click('button[title="Graph View"]');
      await browser.waitUntil(async () => browser.execute(() => {
        const canvas = document.querySelector('.graph-panel canvas');
        return !!canvas?.width && !!canvas?.height && /\d/.test(document.querySelector('.graph-stats')?.innerText ?? '');
      }), { timeout: 60_000, timeoutMsg: 'graph did not draw' });
      await click('button.graph-close');
      console.log(`graph ${run}: drawn`);
    }
  }

  if (mode === 'all' || mode === 'search-only') {
    for (let run = 1; run <= 5; run += 1) {
      await click('button[title^="Search"]');
      await click('.search-modes button:last-child');
      await browser.execute(() => {
        const input = document.querySelector('input[placeholder="Search notes..."]');
        input.value = 'how to bake bread';
        input.dispatchEvent(new Event('input', { bubbles: true }));
      });
      await browser.waitUntil(async () => browser.execute(() => document.querySelectorAll('.result-title').length > 0), {
        timeout: 60_000, timeoutMsg: 'semantic search returned no results',
      });
      await browser.execute(() => document.querySelector('input[placeholder="Search notes..."]')
        .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
      console.log(`semantic ${run}: results painted`);
    }
  }
} finally {
  await browser.deleteSession();
}
