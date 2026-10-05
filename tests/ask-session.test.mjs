import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { get, writable } from 'svelte/store';
import { transformWithEsbuild } from 'vite';

const load = async (file) => {
  const source = await readFile(new URL(`../src/lib/utils/${file}`, import.meta.url), 'utf8');
  return (await transformWithEsbuild(source, file, { loader: 'ts', format: 'esm', target: 'esnext' })).code;
};
const toUrl = (code) => `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`;
const askUrl = toUrl(await load('ask.ts'));
const { createAskSession } = await import(toUrl((await load('ask-session.ts')).replace(/["']\$lib\/utils\/ask["']/, JSON.stringify(askUrl))));

const plan = (count, overrides = {}) => ({
  sources: Array.from({ length: count }, (_, index) => ({ number: index + 1, path: `/vault/${index}.md`, title: `Note ${index}` })),
  relatedNotes: count,
  unread: [],
  queuedNotes: 0,
  ...overrides,
});

/** A session over fakes: questions wait until the test resolves them, and events are pushed by hand. */
function harness({ listenFails = false } = {}) {
  const answers = writable([]);
  const pending = new Map();
  const cancelled = [];
  let handlers = [];
  let next = 0;
  const session = createAskSession({
    answers,
    askNotes: (_question, _category, id) => new Promise((resolve, reject) => pending.set(id, { resolve, reject })),
    cancelAi: async (id) => void cancelled.push(id),
    listen: async (handler) => {
      if (listenFails) throw new Error('listener unavailable');
      handlers.push(handler);
      return () => (handlers = handlers.filter((item) => item !== handler));
    },
    newId: () => `q${++next}`,
    now: () => 1_000,
  });
  const emit = (request_id, event_type, extra = {}) =>
    handlers.forEach((handler) => handler({ request_id, event_type, text: null, error: null, ...extra }));
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
  return { session, answers, pending, cancelled, emit, settle, listeners: () => handlers.length };
}

const status = (answers, id) => get(answers).find((answer) => answer.id === id)?.status;

test('an answer moves through reading, thinking, and answering to done', async () => {
  const h = harness();
  const asked = h.session.ask('What about coffee?', undefined, '/vault');
  await h.settle();
  assert.equal(status(h.answers, 'q1'), 'searching');

  h.pending.get('q1').resolve(plan(2));
  await asked;
  assert.equal(status(h.answers, 'q1'), 'reading');
  h.emit('q1', 'thinking');
  assert.equal(status(h.answers, 'q1'), 'thinking');
  h.emit('q2', 'text', { text: 'not mine' });
  h.emit('q1', 'text', { text: 'Coffee [1]' });
  h.emit('q1', 'done');

  const [answer] = get(h.answers);
  assert.equal(answer.status, 'done');
  assert.equal(answer.text, 'Coffee [1]');
  assert.equal(h.listeners(), 0);
});

test('a stream that finishes before the plan arrives still gets its sources', async () => {
  const h = harness();
  const asked = h.session.ask('q', undefined, '/vault');
  await h.settle();
  h.emit('q1', 'text', { text: 'fast' });
  h.emit('q1', 'done');
  h.pending.get('q1').resolve(plan(1));
  await asked;

  const [answer] = get(h.answers);
  assert.equal(answer.status, 'done');
  assert.equal(answer.plan.sources.length, 1);
});

test('no related note ends the question without a model call', async () => {
  const h = harness();
  const asked = h.session.ask('telescope?', undefined, '/vault');
  await h.settle();
  h.pending.get('q1').resolve(plan(0, { relatedNotes: 0 }));
  await asked;

  assert.equal(status(h.answers, 'q1'), 'no-match');
  assert.equal(h.listeners(), 0);
});

test('Stop during retrieval cancels the request and ignores what comes after', async () => {
  const h = harness();
  const asked = h.session.ask('q', undefined, '/vault');
  await h.settle();
  h.session.stop('q1');
  assert.deepEqual(h.cancelled, ['q1']);
  h.pending.get('q1').resolve(plan(2));
  await asked;
  await h.settle();

  assert.equal(status(h.answers, 'q1'), 'stopped');
  assert.deepEqual(h.cancelled, ['q1', 'q1']);
  assert.equal(h.listeners(), 0);
});

test('a vault switch drops the old answers and stops the running one', async () => {
  const h = harness();
  const first = h.session.ask('one', undefined, '/a');
  await h.settle();
  h.pending.get('q1').resolve(plan(1));
  await first;
  h.emit('q1', 'done');
  void h.session.ask('two', undefined, '/a');
  await h.settle();

  h.session.vaultChanged('/b');
  await h.settle();

  assert.deepEqual(get(h.answers), []);
  assert.deepEqual(h.cancelled, ['q2']);
  assert.equal(h.listeners(), 0);
  h.session.vaultChanged('/a');
  assert.deepEqual(get(h.answers), []);
});

test('a fourth question drops the oldest answer and stops it if still running', async () => {
  const h = harness();
  for (const question of ['one', 'two', 'three', 'four']) {
    void h.session.ask(question, undefined, '/vault');
    await h.settle();
  }

  assert.deepEqual(get(h.answers).map((answer) => answer.question), ['four', 'three', 'two']);
  assert.deepEqual(h.cancelled, ['q1']);
});

test('a listener that cannot start shows an error instead of searching forever', async () => {
  const h = harness({ listenFails: true });
  await h.session.ask('q', undefined, '/vault');

  const [answer] = get(h.answers);
  assert.equal(answer.status, 'error');
  assert.match(answer.error, /listener unavailable/);
});

test('a stream error keeps the partial answer and shows why it stopped', async () => {
  const h = harness();
  const asked = h.session.ask('q', undefined, '/vault');
  await h.settle();
  h.pending.get('q1').resolve(plan(1));
  await asked;
  h.emit('q1', 'text', { text: 'partial' });
  h.emit('q1', 'error', { error: 'The connection closed before the answer finished.' });

  const [answer] = get(h.answers);
  assert.equal(answer.status, 'error');
  assert.equal(answer.text, 'partial');
  assert.match(answer.error, /closed before/);
});
