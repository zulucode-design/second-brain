import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/notion-settings.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'notion-settings.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { describeSummary, describeSkipped, nextStep } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

const summary = (overrides = {}) => ({
  created: 0,
  updated: 0,
  moved: 0,
  trashed: 0,
  up_to_date: 0,
  failed: 0,
  skipped: {},
  ...overrides
});

const status = (overrides = {}) => ({
  enabled: true,
  publishing: false,
  progress: null,
  connected: true,
  connection_name: null,
  setup_complete: true,
  poll_minutes: 5,
  last_run: null,
  last_summary: null,
  last_error: null,
  failing_notes: 0,
  ...overrides
});

test('a run that changed nothing reads as healthy, not as nothing happening', () => {
  // This is the outcome every five minutes; it must not look like a stalled publisher.
  assert.equal(describeSummary(summary({ up_to_date: 40 })), 'Everything is up to date.');
});

test('a run lists only what it actually did', () => {
  assert.equal(
    describeSummary(summary({ created: 3, moved: 1 })),
    '3 published, 1 moved.'
  );
});

test('failures are always mentioned', () => {
  assert.match(describeSummary(summary({ created: 2, failed: 1 })), /1 failed/);
});

test('nothing skipped shows no line at all', () => {
  assert.equal(describeSkipped(summary()), null);
  assert.equal(describeSkipped(summary({ skipped: { uncategorised: 0 } })), null);
});

test('skipped notes always say why', () => {
  // "3 notes not published" with no reason leaves the user with nowhere to look.
  assert.equal(
    describeSkipped(summary({ skipped: { uncategorised: 2, conflict_copy: 1 } })),
    '3 notes not published: 2 uncategorised, 1 sync conflict copies.'
  );
});

test('a single skipped note is not pluralised', () => {
  assert.equal(
    describeSkipped(summary({ skipped: { uncategorised: 1 } })),
    '1 note not published: 1 uncategorised.'
  );
});

test('an unknown skip reason is shown rather than dropped', () => {
  assert.match(describeSkipped(summary({ skipped: { something_new: 2 } })), /something_new/);
});

test('with no token the only next step is to connect', () => {
  assert.equal(nextStep(status({ connected: false, setup_complete: false })), 'Connect an integration token to begin.');
});

test('a connected machine with no databases is asked to choose a parent page', () => {
  assert.match(nextStep(status({ setup_complete: false })), /Choose the Notion page/);
});

test('a failed run surfaces its reason as the next step', () => {
  assert.equal(
    nextStep(status({ last_error: 'Notion rejected the connection: API token is invalid.' })),
    'Notion rejected the connection: API token is invalid.'
  );
});

test('a healthy, set-up machine has no next step', () => {
  assert.equal(nextStep(status()), null);
});
