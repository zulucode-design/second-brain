#!/usr/bin/env node
// Asks the Ask fixture's questions (#8) in a real app build and records what came back.
//
//   node scripts/ask-run.mjs --app <binary> [--model gpt-oss:20b-cloud] [--root <dir>] [--ssh sb-windows]
//
// Runs on Fedora under tauri-driver, with the app's config and data in a fresh run root, so the
// real profile is never touched. The desktop's Ollama serves both the embeddings and the chat
// model; it listens on loopback only, so the app reaches it through an `ssh -L` tunnel.
// The answers are judged by a reader: the trace records each one beside what the fixture expects.

import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { generate, QUESTIONS } from './ask-fixture.mjs';

const APP_IDENTIFIER = 'io.github.zulucodedesign.SecondBrain';
const DRIVER_PORT = 4444;
const NATIVE_PORT = 4447;
const OLLAMA_PORT = 11435;
const ANSWER_TIMEOUT = 5 * 60_000;

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

function options(args) {
  const parsed = { model: 'gpt-oss:20b-cloud', ssh: 'sb-windows' };
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index].replace(/^--/, '');
    if (!['app', 'model', 'root', 'ssh'].includes(key) || args[index + 1] === undefined) {
      throw new Error(`usage: ask-run.mjs --app <binary> [--model <name>] [--root <dir>] [--ssh <host>]`);
    }
    parsed[key] = args[index + 1];
  }
  if (!parsed.app) throw new Error('--app <binary> is required');
  parsed.root ??= join(homedir(), 'sb-ask-run', new Date().toISOString().replaceAll(':', '-'));
  return parsed;
}

function setUp(root, model) {
  const vault = join(root, 'vault');
  const configHome = join(root, 'config');
  const dataHome = join(root, 'data');
  mkdirSync(root, { recursive: true });
  // Not recursive: a reused root would mix runs, so it fails here rather than being cleaned.
  mkdirSync(vault);
  mkdirSync(join(vault, '.helixnotes'));
  const vaultId = randomUUID();
  writeFileSync(join(vault, '.helixnotes', 'vault_id'), vaultId, { flag: 'wx' });
  generate(vault);
  mkdirSync(join(configHome, APP_IDENTIFIER), { recursive: true });
  mkdirSync(dataHome);
  writeFileSync(join(configHome, APP_IDENTIFIER, 'config.json'), JSON.stringify({
    vault: { path: vault, name: 'Ask Run', vault_id: vaultId },
    active_vault: vault,
    ai_provider: 'ollama',
    ai_model: model,
    ai_models: { ollama: model },
    ollama_base_url: `http://127.0.0.1:${OLLAMA_PORT}`,
    backup_enabled: false,
    close_to_tray: false,
  }, null, 2));
  return { vault, configHome, dataHome };
}

async function waitForPort(port, child, name) {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    if (child.exitCode !== null) throw new Error(`${name} exited ${child.exitCode}; is port ${port} taken?`);
    try {
      await fetch(`http://127.0.0.1:${port}/`);
      return;
    } catch {
      await sleep(500);
    }
  }
  throw new Error(`${name} did not answer on port ${port}`);
}

// Tauri commands straight from the page, for state the UI does not show.
function invoke(browser, command, args = {}) {
  return browser.executeAsync((name, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(name, payload).then(done, (error) => done({ error: String(error) }));
  }, command, args);
}

async function waitForIndex(browser, notes) {
  const deadline = Date.now() + 10 * 60_000;
  let status;
  while (Date.now() < deadline) {
    status = await invoke(browser, 'get_semantic_status');
    if (status.indexed_notes >= notes && status.queued_notes === 0) return status;
    await sleep(2_000);
  }
  throw new Error(`index did not finish: ${JSON.stringify(status)}`);
}

// WebKitWebDriver rejects key actions and element-click, so input goes through the DOM; the
// app's handlers are plain listeners and see it the same way.
async function ask(browser, question) {
  await browser.execute(() => {
    if (!document.querySelector('.search-panel')) {
      window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyF', key: 'F', ctrlKey: true, shiftKey: true, bubbles: true }));
    }
  });
  await browser.$('.search-panel').waitForExist({ timeout: 10_000 });
  await browser.execute(() => [...document.querySelectorAll('.search-modes button')].find((b) => b.innerText.trim() === 'Ask').click());
  await browser.execute((text) => {
    const input = document.querySelector('.search-panel input');
    input.value = text;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  }, question);

  const started = Date.now();
  await browser.waitUntil(
    () => browser.execute((text) => {
      const open = document.querySelector('.ask-answer.open');
      return open?.querySelector('.ask-question')?.innerText.trim() === text && !open.querySelector('.ask-stop');
    }, question),
    { timeout: ANSWER_TIMEOUT, interval: 500, timeoutMsg: `no finished answer within ${ANSWER_TIMEOUT / 1000}s` },
  );
  const seconds = Math.round((Date.now() - started) / 1000);

  return { seconds, ...(await browser.execute(() => {
    const open = document.querySelector('.ask-answer.open');
    const text = (selector) => open.querySelector(selector)?.innerText.trim() ?? null;
    const html = open.querySelector('.ask-text')?.innerHTML ?? '';
    return {
      answer: text('.ask-text'),
      error: text('.ask-error'),
      empty: text('.ask-empty'),
      warning: text('.ask-warning'),
      stopped: text('.ask-note'),
      coverage: [...open.querySelectorAll('.ask-coverage')].map((node) => node.innerText.trim()),
      sources: [...open.querySelectorAll('.source:not(.unread) .source-title')].map((node) => node.innerText.trim()),
      unread: [...open.querySelectorAll('.source.unread .source-title')].map((node) => node.innerText.trim()),
      citations: [...open.querySelectorAll('.ask-text .citation')].map((node) => Number(node.dataset.source)),
      // Prompt-injection check: an answer must never render an image, a link, or raw HTML.
      rendersLinkOrImage: /<img|<a[\s>]|href=|src=/.test(html),
    };
  })) };
}

async function main() {
  const { app, model, root, ssh } = options(process.argv.slice(2));
  const paths = setUp(resolve(root), model);
  const children = [];
  try {
    const tunnel = spawn('ssh', ['-N', '-o', 'ExitOnForwardFailure=yes', '-L', `127.0.0.1:${OLLAMA_PORT}:127.0.0.1:11434`, ssh], { stdio: 'ignore' });
    children.push(tunnel);
    await waitForPort(OLLAMA_PORT, tunnel, 'Ollama tunnel');
    const driver = spawn('tauri-driver', ['--port', String(DRIVER_PORT), '--native-port', String(NATIVE_PORT)], {
      env: { ...process.env, XDG_CONFIG_HOME: paths.configHome, XDG_DATA_HOME: paths.dataHome },
      stdio: 'ignore',
    });
    children.push(driver);
    await waitForPort(DRIVER_PORT, driver, 'tauri-driver');

    const { remote } = await import('webdriverio');
    const browser = await remote({
      hostname: '127.0.0.1', port: DRIVER_PORT, logLevel: 'warn', connectionRetryCount: 0,
      capabilities: { 'tauri:options': { application: resolve(app) } },
    });
    try {
      await browser.setWindowSize(1280, 860);
      await browser.$('button=New Note').waitForDisplayed({ timeout: 60_000 });
      const index = await waitForIndex(browser, 40);
      const results = [];
      for (const { question, expect, cites } of QUESTIONS) {
        const result = await ask(browser, question);
        results.push({ question, expect, cites, missingCites: cites.filter((title) => !result.sources.includes(title)), ...result });
        console.log(`${result.seconds}s  ${question}`);
      }
      const trace = { when: new Date().toISOString(), app: resolve(app), model, index, results };
      const out = join(root, 'ask-run.json');
      writeFileSync(out, JSON.stringify(trace, null, 2));
      console.log(out);
    } finally {
      await browser.deleteSession().catch(() => {});
    }
  } finally {
    for (const child of children) child.kill();
  }
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
