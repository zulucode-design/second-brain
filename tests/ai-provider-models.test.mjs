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
const { modelForProvider, rememberAiModels } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

// Walk the settings switch the way SettingsPanel does: pick the restored model, then save.
function switchTo(config, provider, model = modelForProvider(config.ai_models, provider)) {
  return {
    ai_provider: provider,
    ai_model: model,
    ai_models: rememberAiModels(config, provider, model)
  };
}

test('switching away from Ollama and back restores the custom model', () => {
  let config = { ai_provider: null, ai_model: 'claude-sonnet-4-6', ai_models: {} };
  config = switchTo(config, 'ollama', 'gemma3');
  config = switchTo(config, 'openai');
  assert.equal(config.ai_model, 'gpt-5.5');
  config = switchTo(config, 'ollama');
  assert.equal(config.ai_model, 'gemma3');
});

test('each provider keeps an independent model', () => {
  let config = { ai_provider: null, ai_model: 'claude-sonnet-4-6', ai_models: {} };
  const chosen = {
    ollama: 'gemma3',
    anthropic: 'claude-opus-4-8',
    openai: 'gpt-5-mini',
    openai_compatible: 'local-llama'
  };
  for (const [provider, model] of Object.entries(chosen)) config = switchTo(config, provider, model);
  for (const [provider, model] of Object.entries(chosen)) {
    config = switchTo(config, provider);
    assert.equal(config.ai_model, model, provider);
  }
});

test('a provider with no remembered model starts with its default', () => {
  assert.equal(modelForProvider({}, 'ollama'), 'gemma3:4b');
  assert.equal(modelForProvider({}, 'openai_compatible'), '');
});

test('an older config keeps its shared model once the user switches away', () => {
  let config = { ai_provider: 'ollama', ai_model: 'gemma3', ai_models: {} };
  config = switchTo(config, 'anthropic');
  config = switchTo(config, 'ollama');
  assert.equal(config.ai_model, 'gemma3');
});
