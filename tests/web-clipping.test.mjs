import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const webClippingSource = await readFile(
  new URL('../src/lib/utils/web-clipping.ts', import.meta.url),
  'utf8'
);
const webClippingResult = await transformWithEsbuild(
  webClippingSource,
  'web-clipping.ts',
  {
    loader: 'ts',
    format: 'esm',
    target: 'esnext'
  }
);
const {
  canSubmitWebClip,
  cleanClipUrlInput,
  webClipFailureMessage
} = await import(`data:text/javascript;base64,${Buffer.from(webClippingResult.code).toString('base64')}`);

test('web clipping requires an actual pasted URL before category submission', () => {
  assert.equal(canSubmitWebClip('   '), false);
  assert.equal(canSubmitWebClip(' https://example.com/article '), true);
  assert.equal(cleanClipUrlInput(' https://example.com/article '), 'https://example.com/article');
});

test('web clipping displays backend failure reasons without losing the fallback', () => {
  assert.equal(
    webClipFailureMessage(new Error('The web page took too long to respond.')),
    'The web page took too long to respond.'
  );
  assert.equal(webClipFailureMessage(''), 'Could not clip that web page.');
});
