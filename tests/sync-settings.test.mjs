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
  /setSyncSettings/,
  /testSyncConnection/,
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

test('production command, config, API, scheduler, and UI paths retain no WebDAV contracts', async () => {
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

test('the selected sidecar is bundled and every app launch path prepares it', async () => {
  const overlay = JSON.parse(await source('src-tauri/tauri.sidecar.conf.json'));
  assert.deepEqual(overlay.bundle.externalBin, ['binaries/syncthing']);

  const packageJson = JSON.parse(await source('package.json'));
  assert.match(packageJson.scripts['tauri:dev'], /sidecar:prepare/);
  assert.match(packageJson.scripts['tauri:dev'], /tauri\.sidecar\.conf\.json/);
  assert.match(packageJson.scripts['tauri:build'], /sidecar:prepare/);
  assert.match(packageJson.scripts['tauri:build'], /tauri\.sidecar\.conf\.json/);

  const workflow = await source('.github/workflows/verify.yml');
  assert.match(workflow, /pnpm sidecar:prepare/);
  assert.match(workflow, /externalBin/);
  assert.match(workflow, /syncthing-x86_64-pc-windows-msvc\.exe --version/);
});

test('sync remains an explicit guarded batch rather than ambient Tailnet access', async () => {
  const backend = await source('src-tauri/src/sync_sidecar.rs');
  assert.match(backend, /autoAcceptFolders"\s*:\s*false/);
  assert.match(backend, /globalAnnounceEnabled/);
  assert.match(backend, /create_pre_sync_backup/);
  assert.match(backend, /bulk_mutation/);
  assert.match(backend, /FolderPauseGuard/);
  assert.match(backend, /reconcile_bulk_projections/);

  const settings = await source('src/lib/components/SettingsPanel.svelte');
  assert.match(settings, /Pair explicitly/);
  assert.match(settings, /Vault ID from the other machine/);
  assert.match(settings, /Conflict copies stay out of notes/);
});

test('a delayed local attachment is presented as not synced yet', async () => {
  const editor = await source('src/lib/components/Editor.svelte');
  assert.match(editor, /Attachment not synced yet/);
  assert.match(editor, /listen\('sync-done', retryDelayedAttachments\)/);
});
