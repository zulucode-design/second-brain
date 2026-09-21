import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const { acquireControllerLock, harnessConfig, pairedSyncthingConfig, redactTrace } = await import(
  new URL('../scripts/alpha-harness.mjs', import.meta.url)
);

test('harness config changes only active vault and backup ownership', () => {
  const original = {
    vaults: [{ path: '/real', name: 'Real', vault_id: 'real-id', notion: { enabled: true } }],
    active_vault: '/real',
    active_bookmark_id: 'bookmark',
    backup_location: '/real-backups',
    backup_max_count: 3,
    theme: 'dark',
    ai_provider: 'ollama',
  };
  const next = harnessConfig(original, '/test/vault', 'test-id', '/test/backups');

  assert.deepEqual(original.vaults[0].notion, { enabled: true }, 'input stays untouched');
  assert.deepEqual(next.vaults, [{ path: '/test/vault', name: 'Alpha Harness', vault_id: 'test-id' }]);
  assert.equal(next.active_vault, '/test/vault');
  assert.equal(next.active_bookmark_id, null);
  assert.equal(next.backup_location, '/test/backups');
  assert.equal(next.backup_max_count, 10);
  assert.equal(next.theme, 'dark');
  assert.equal(next.ai_provider, 'ollama');
});

test('pairing config keeps one explicit peer and a paused disposable vault', () => {
  const peerId = 'AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH';
  const original = { devices: [{ deviceID: 'OLD' }], folders: [{ id: 'old' }], options: { urAccepted: 1 } };
  const next = pairedSyncthingConfig(original, {
    vaultId: 'vault-id',
    vaultPath: 'D:\\SecondBrainTest\\sb88\\runs\\1\\vault',
    peerId,
    peerName: 'Fedora',
    peerIp: '100.64.0.1',
    localIp: '100.64.0.2',
  });

  assert.equal(original.devices[0].deviceID, 'OLD', 'input stays untouched');
  assert.deepEqual(next.devices, [{
    deviceID: peerId,
    name: 'Fedora',
    addresses: ['tcp://100.64.0.1:22000'],
    autoAcceptFolders: false,
  }]);
  assert.equal(next.folders[0].id, 'vault-id');
  assert.equal(next.folders[0].paused, true);
  assert.equal(next.folders[0].devices[0].deviceID, peerId);
  assert.deepEqual(next.options.listenAddresses, ['tcp://100.64.0.2:22000']);
  for (const key of ['globalAnnounceEnabled', 'localAnnounceEnabled', 'relaysEnabled', 'natEnabled', 'crashReportingEnabled']) {
    assert.equal(next.options[key], false);
  }
  assert.equal(next.options.urAccepted, -1);
});

test('pairing refuses a malformed peer identity', () => {
  assert.throws(
    () => pairedSyncthingConfig({}, {
      vaultId: 'vault-id', vaultPath: '/vault', peerId: 'anything', peerName: 'peer',
      peerIp: '100.64.0.1', localIp: '100.64.0.2',
    }),
    /invalid Syncthing device ID/,
  );
});

test('controller lock excludes a concurrent run and replaces a stale owner', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'alpha-harness-lock-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));

  const release = acquireControllerLock(root);
  assert.throws(() => acquireControllerLock(root), /another alpha harness owns/);
  release();

  writeFileSync(
    join(root, 'alpha-harness.lock.json'),
    JSON.stringify({ pid: 2_147_483_647, token: 'stale' }),
  );
  const releaseRecovered = acquireControllerLock(root);
  releaseRecovered();
});

test('Windows launch task targets the logged-in desktop identity', () => {
  const script = readFileSync(new URL('../scripts/windows/alpha-harness.ps1', import.meta.url), 'utf8');
  assert.match(script, /Get-CimInstance Win32_ComputerSystem\)\.UserName/);
  assert.doesNotMatch(script, /USERDOMAIN|USERNAME/);
});

test('public traces drop account names, home paths, and host names', () => {
  const line = JSON.stringify({
    vault: `${homedir()}/sb88/runs/x/fedora/vault`,
    machineLink: 'C:\\Users\\Someone\\AppData\\Local\\helixnotes\\vaults\\id',
    host: 'DESKTOP-EXAMPLE',
    matching: 5000,
  });
  const redacted = redactTrace(line);
  assert.deepEqual(JSON.parse(redacted), {
    vault: '~/sb88/runs/x/fedora/vault',
    machineLink: '%USERPROFILE%\\AppData\\Local\\helixnotes\\vaults\\id',
    matching: 5000,
  });
  assert.deepEqual(JSON.parse(redactTrace(JSON.stringify({ matching: 1, host: 'last' }))), { matching: 1 });
});
