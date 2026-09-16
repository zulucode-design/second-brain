import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), 'utf8');

test('packages expose the independent Second Brain identity', async () => {
  const [packageJson, cargo, tauri, windows, desktop] = await Promise.all([
    read('package.json').then(JSON.parse),
    read('src-tauri/Cargo.toml'),
    read('src-tauri/tauri.conf.json').then(JSON.parse),
    read('src-tauri/tauri.windows.conf.json').then(JSON.parse),
    read('src-tauri/linux/io.github.zulucodedesign.SecondBrain.desktop'),
  ]);

  assert.equal(packageJson.name, 'second-brain');
  assert.equal(packageJson.version, '0.1.0-alpha.1');
  assert.equal(tauri.productName, 'Second Brain');
  assert.equal(tauri.version, '0.1.0-alpha.1');
  assert.equal(tauri.identifier, 'io.github.zulucodedesign.SecondBrain');
  assert.equal(tauri.app.windows[0].title, 'Second Brain');
  assert.equal(windows.app.windows[0].title, 'Second Brain');
  assert.match(cargo, /^name = "second-brain"$/m);
  assert.match(cargo, /^version = "0.1.0-alpha.1"$/m);
  assert.match(desktop, /^Name=Second Brain$/m);
  assert.match(desktop, /^Exec=second-brain$/m);
  assert.match(desktop, /^Icon=second-brain$/m);
  assert.match(desktop, /^StartupWMClass=second-brain$/m);
});

test('the inherited updater is absent from packages and runtime', async () => {
  const files = await Promise.all([
    read('package.json'),
    read('src-tauri/Cargo.toml'),
    read('src-tauri/tauri.conf.json'),
    read('src-tauri/capabilities/desktop.json'),
    read('src-tauri/src/lib.rs'),
    read('src/routes/+layout.svelte'),
    read('src/lib/stores/app.ts'),
    read('src/lib/components/SettingsPanel.svelte'),
  ]);
  const source = files.join('\n');

  assert.doesNotMatch(source, /tauri-apps\/plugin-updater|tauri-plugin-updater/);
  assert.doesNotMatch(source, /helixnotes\.com\/latest\.json/);
  assert.equal(JSON.parse(files[2]).bundle.createUpdaterArtifacts, false);
});

test('the stable app id and legacy config location preserve existing credentials and settings', async () => {
  const [secretStore, commands] = await Promise.all([
    read('src-tauri/src/secret_store.rs'),
    read('src-tauri/src/commands.rs'),
  ]);

  assert.match(secretStore, /const SERVICE: &str = "io\.github\.zulucodedesign\.SecondBrain";/);
  assert.match(commands, /config_dir\.join\("helixnotes"\)/);
});
