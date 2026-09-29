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
const config = (overrides) => ({ ai_provider: null, ai_model: 'claude-sonnet-4-6', ai_models: {}, ...overrides });

test('switching to a provider restores its remembered model', () => {
  const ai_models = { ollama: 'gemma3', openai: 'gpt-5-mini', anthropic: 'claude-opus-4-8', openai_compatible: 'local-llama' };
  for (const [provider, model] of Object.entries(ai_models)) {
    assert.equal(modelForProvider(config({ ai_models }), provider), model, provider);
  }
});

test('a provider with no remembered model starts with its default', () => {
  assert.equal(modelForProvider(config({ ai_models: { openai: 'gpt-5-mini' } }), 'ollama'), 'gemma3:4b');
  assert.equal(modelForProvider(config(), 'anthropic'), 'claude-sonnet-4-6');
  assert.equal(modelForProvider(config(), 'openai_compatible'), '');
});

test('the active provider takes ai_model, which is all an older config remembers', () => {
  const older = config({ ai_provider: 'ollama', ai_model: 'gemma3' });
  assert.equal(modelForProvider(older, 'ollama'), 'gemma3');
  assert.equal(modelForProvider(older, 'openai'), 'gpt-5.5');
});

test('a cleared model falls back to the default instead of staying empty', () => {
  assert.equal(modelForProvider(config({ ai_models: { ollama: '' } }), 'ollama'), 'gemma3:4b');
  assert.equal(modelForProvider(config({ ai_provider: 'ollama', ai_model: '' }), 'ollama'), 'gemma3:4b');
});

test('a model sent in a save that has not returned wins over the stale saved config', () => {
  // Typed llama3 over gemma3, switched away and back before the first save returned.
  const stale = config({ ai_provider: 'ollama', ai_model: 'gemma3' });
  assert.equal(modelForProvider(stale, 'ollama', { ollama: 'llama3' }), 'llama3');
  assert.equal(modelForProvider(stale, 'ollama', { openai: 'gpt-5-mini' }), 'gemma3');
});
