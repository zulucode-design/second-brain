import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(
  new URL('../src/lib/utils/sync-settings.ts', import.meta.url),
  'utf8'
);
const { code } = await transformWithEsbuild(source, 'sync-settings.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { withSyncSettings } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`
);

const vault = () => ({
  path: '/vaults/brain',
  name: 'brain',
  sync_provider: 'webdav',
  credentials: { webdav: { url: 'https://old.example.com', username: 'old', password: 'old' } },
  schedule: { on_open: false, on_change: false, interval_minutes: 0, last_sync_time: '2026-08-01T00:00:00Z' }
});

const edited = {
  provider: 'webdav',
  url: 'https://new.example.com/dav',
  username: 'nicolas',
  password: 'hunter2',
  onOpen: true,
  onChange: true,
  intervalMinutes: 15
};

test('edits land where the readers look, not as pre-grouping flat keys', () => {
  const updated = withSyncSettings(vault(), edited);

  // These are the exact paths AppLayout and SettingsPanel read.
  assert.equal(updated.credentials.webdav.url, 'https://new.example.com/dav');
  assert.equal(updated.credentials.webdav.username, 'nicolas');
  assert.equal(updated.credentials.webdav.password, 'hunter2');
  assert.equal(updated.schedule.on_open, true);
  assert.equal(updated.schedule.on_change, true);
  assert.equal(updated.schedule.interval_minutes, 15);
  assert.equal(updated.sync_provider, 'webdav');

  // The layout written before the grouping must not come back: a reader that found
  // `webdav_url` here would be reading a value nothing else maintains.
  for (const dead of [
    'webdav_url',
    'webdav_username',
    'webdav_password',
    'sync_on_open',
    'sync_on_change',
    'sync_interval_minutes'
  ]) {
    assert.ok(!(dead in updated), `${dead} should not be written`);
  }
});

test('last_sync_time is the backend\'s and survives an edit', () => {
  const updated = withSyncSettings(vault(), edited);
  assert.equal(updated.schedule.last_sync_time, '2026-08-01T00:00:00Z');
});

test('another provider\'s credentials are left alone', () => {
  const withNotion = vault();
  withNotion.credentials.notion = { token: 'secret-token' };

  const updated = withSyncSettings(withNotion, edited);

  assert.deepEqual(updated.credentials.notion, { token: 'secret-token' });
  assert.equal(updated.credentials.webdav.url, 'https://new.example.com/dav');
});

test('the original vault entry is not mutated', () => {
  const original = vault();
  const updated = withSyncSettings(original, edited);

  assert.equal(original.credentials.webdav.url, 'https://old.example.com');
  assert.notEqual(updated, original);
});

test('a vault with no sync settings yet gets the nested shape', () => {
  const bare = { path: '/vaults/new', name: 'new' };

  const updated = withSyncSettings(bare, edited);

  assert.equal(updated.credentials.webdav.url, 'https://new.example.com/dav');
  assert.equal(updated.schedule.interval_minutes, 15);
  assert.equal(updated.schedule.last_sync_time, undefined);
});
