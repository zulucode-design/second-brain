import assert from 'node:assert/strict';
import { access, readFile } from 'node:fs/promises';
import test from 'node:test';

const root = new URL('../', import.meta.url);

async function source(path) {
  return readFile(new URL(path, root), 'utf8');
}

const productionFiles = [
  'src-tauri/src/commands.rs',
  'src-tauri/src/lib.rs',
  'src-tauri/src/state.rs',
  'src-tauri/src/types.rs',
  'src/lib/api.ts',
  'src/lib/types.ts',
  'src/lib/stores/app.ts',
  'src/lib/components/AppLayout.svelte',
  'src/lib/components/SettingsPanel.svelte',
  'src/lib/components/TitleBar.svelte',
];

const forbiddenProductionContracts = [
  /WebdavClient/,
  /WebdavConfig/,
  /mod sync;/,
  /set_sync_settings/,
  /test_sync_connection/,
  /sync_now/,
  /setSyncSettings/,
  /testSyncConnection/,
  /syncNow/,
  /sync-progress/,
  /sync-done/,
  /sync-error/,
  /sync-test-result/,
  /sync_provider/,
  /webdav_url/,
  /WebdavCredentials/,
];

test('the WebDAV client module is absent from the production crate', async () => {
  await assert.rejects(
    access(new URL('src-tauri/src/sync.rs', root)),
    /ENOENT/,
  );
});

test('production command, config, API, scheduler, and UI paths cannot invoke WebDAV', async () => {
  for (const path of productionFiles) {
    const contents = await source(path);
    for (const forbidden of forbiddenProductionContracts) {
      assert.doesNotMatch(contents, forbidden, `${path} retained ${forbidden}`);
    }
  }
});

test('the removed frontend configuration helper cannot be imported', async () => {
  await assert.rejects(
    access(new URL('src/lib/utils/sync-settings.ts', root)),
    /ENOENT/,
  );
});
