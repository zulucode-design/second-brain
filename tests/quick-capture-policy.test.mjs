import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/quick-capture-policy.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'quick-capture-policy.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { captureAction, missingField } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

const state = (overrides) => ({ title: 'Title', body: 'Body', field: 'title', selected: 0, ...overrides });
const writing = (key, overrides, event = {}) => captureAction('writing', { key, ...event }, state(overrides));

test('Tab and Shift+Tab move to the other field and wrap (#213)', () => {
  for (const shiftKey of [false, true]) {
    assert.deepEqual(writing('Tab', { field: 'title' }, { shiftKey }), { type: 'focus', field: 'body' });
    assert.deepEqual(writing('Tab', { field: 'body' }, { shiftKey }), { type: 'focus', field: 'title' });
  }
});

test('Tab never opens the category picker', () => {
  for (const field of ['title', 'body']) {
    assert.notEqual(writing('Tab', { field }).type, 'choose');
  }
});

test('Enter in the title moves to the body; in the body it is a newline', () => {
  assert.deepEqual(writing('Enter', { field: 'title' }), { type: 'focus', field: 'body' });
  assert.deepEqual(writing('Enter', { field: 'body' }), { type: 'insert' });
});

test('Enter that confirms an input-method composition is left to the field', () => {
  assert.deepEqual(writing('Enter', { field: 'title' }, { isComposing: true }), { type: 'insert' });
});

test('Ctrl+Enter opens the picker from either field once both are filled', () => {
  for (const field of ['title', 'body']) {
    assert.equal(writing('Enter', { field }, { ctrlKey: true }).type, 'choose');
    assert.equal(writing('Enter', { field }, { metaKey: true }).type, 'choose');
  }
});

test('Ctrl+Enter with a field empty names that field, title first', () => {
  assert.deepEqual(writing('Enter', { title: '  ' }, { ctrlKey: true }), { type: 'missing', field: 'title' });
  assert.deepEqual(writing('Enter', { body: '\n \t' }, { ctrlKey: true }), { type: 'missing', field: 'body' });
  assert.deepEqual(writing('Enter', { title: '', body: '' }, { ctrlKey: true }), { type: 'missing', field: 'title' });
});

test('missingField treats whitespace as empty', () => {
  assert.equal(missingField(' ', 'b'), 'title');
  assert.equal(missingField('t', ' \n'), 'body');
  assert.equal(missingField('t', 'b'), null);
});

test('Esc asks before discarding when either field has text', () => {
  assert.equal(writing('Escape', { title: 'x', body: '' }).type, 'confirmDiscard');
  assert.equal(writing('Escape', { title: '', body: 'x' }).type, 'confirmDiscard');
  assert.equal(writing('Escape', { title: ' ', body: '\n' }).type, 'dismiss');
});

test('digits are typed while writing and pick a category while choosing', () => {
  assert.deepEqual(writing('2', {}), { type: 'insert' });
  assert.deepEqual(captureAction('choosing', { key: '2' }, state()), { type: 'save', category: 'Areas' });
});
