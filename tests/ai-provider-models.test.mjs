import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/ai-provider.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'ai-provider.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { modelForProvider } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

// The backend records the map (AppConfig::select_ai_model); this only picks from it.
test('switching to a provider restores its remembered model', () => {
  const models = { ollama: 'gemma3', openai: 'gpt-5-mini', anthropic: 'claude-opus-4-8', openai_compatible: 'local-llama' };
  for (const [provider, model] of Object.entries(models)) {
    assert.equal(modelForProvider(models, provider), model, provider);
  }
});

test('a provider with no remembered model starts with its default', () => {
  assert.equal(modelForProvider({ openai: 'gpt-5-mini' }, 'ollama'), 'gemma3:4b');
  assert.equal(modelForProvider({}, 'anthropic'), 'claude-sonnet-4-6');
  assert.equal(modelForProvider({}, 'openai_compatible'), '');
});

test('a cleared model falls back to the default instead of staying empty', () => {
  assert.equal(modelForProvider({ ollama: '' }, 'ollama'), 'gemma3:4b');
});
