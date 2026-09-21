import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const {
  acquireControllerLock, harnessConfig, killDue, mutateFixture, pairedSyncthingConfig, recoveryOutcome, redactTrace,
  restoreProgress, treeHash,
} = await import(
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

test('restore gate config turns off scheduled backups and nothing else', () => {
  const original = { backup_enabled: true, theme: 'dark' };
  assert.equal(harnessConfig(original, '/v', 'id', '/b').backup_enabled, true);
  const next = harnessConfig(original, '/v', 'id', '/b', 'restore');
  assert.equal(next.backup_enabled, false);
  assert.equal(next.theme, 'dark');
});

test('vault hash covers metadata, empty folders, and names, and nothing else', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'alpha-harness-hash-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const vault = join(root, 'vault');
  mkdirSync(join(vault, '.helixnotes'), { recursive: true });
  mkdirSync(join(vault, 'Areas'));
  writeFileSync(join(vault, '.helixnotes', 'vault_id'), 'id');
  writeFileSync(join(vault, 'note.md'), 'body');

  const base = treeHash(vault);
  assert.deepEqual({ files: base.files, directories: base.directories }, { files: 2, directories: 2 });
  assert.equal(treeHash(vault).sha256, base.sha256, 'stable');
  writeFileSync(join(vault, '.helixnotes', 'vault_id'), 'other');
  assert.notEqual(treeHash(vault).sha256, base.sha256, 'metadata content counts');
  writeFileSync(join(vault, '.helixnotes', 'vault_id'), 'id');
  rmSync(join(vault, 'Areas'), { recursive: true });
  assert.notEqual(treeHash(vault).sha256, base.sha256, 'an empty folder counts');
});

test('restore progress reads the journal and stage, and kill points wait for them', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'alpha-harness-restore-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const vault = join(root, 'vault');
  mkdirSync(vault);
  assert.deepEqual(restoreProgress(vault), { leftovers: [], phase: null, stagedFiles: 0, rollback: false });
  assert.equal(killDue('journal', restoreProgress(vault), 10), false, 'no restore, no kill');

  writeFileSync(join(root, '.second-brain-restore-journal-1.json'), JSON.stringify({ phase: 'unpacking' }));
  mkdirSync(join(root, '.second-brain-restore-stage-1', 'Areas'), { recursive: true });
  for (let index = 0; index < 5; index += 1) {
    writeFileSync(join(root, '.second-brain-restore-stage-1', 'Areas', `${index}.md`), '');
  }
  const progress = restoreProgress(vault);
  assert.equal(progress.phase, 'unpacking');
  assert.equal(progress.stagedFiles, 5);
  assert.equal(killDue('journal', progress, 10), true);
  assert.equal(killDue('stage-half', progress, 10), true);
  assert.equal(killDue('stage-late', progress, 10), false);
  assert.throws(() => killDue('elsewhere', progress, 10), /unknown kill point/);
});

test('the pre-restore vault differs from the backed-up fixture', async (t) => {
  const { generate, NOTE_COUNT } = await import(new URL('../scripts/sync-fixture.mjs', import.meta.url));
  const root = mkdtempSync(join(tmpdir(), 'alpha-harness-mutate-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, '.helixnotes'));
  writeFileSync(join(root, '.helixnotes', 'vault_id'), 'id');
  generate(root);
  const backedUp = treeHash(root);

  assert.deepEqual(mutateFixture(root), { edited: 500, removed: 500, added: 100 });
  const pre = treeHash(root);
  assert.notEqual(pre.sha256, backedUp.sha256);
  assert.equal(pre.files, backedUp.files - 500 + 100);
  assert.equal(backedUp.files, NOTE_COUNT + 1);
});

test('recovery must land on exactly one whole state', () => {
  assert.equal(recoveryOutcome('a', 'a', 'b'), 'pre-state');
  assert.equal(recoveryOutcome('b', 'a', 'b'), 'post-state');
  assert.equal(recoveryOutcome('c', 'a', 'b'), null);
});
