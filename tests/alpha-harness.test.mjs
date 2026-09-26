import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { deflateRawSync } from 'node:zlib';

const {
  acquireControllerLock, diagnosticLeaks, harnessConfig, killDue, mutateFixture, pairedSyncthingConfig, recoveryOutcome, redactTrace,
  restoreProgress, snapshotVault, treeHash, zipEntries,
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

test('driven gate config turns off scheduled backups and close-to-tray, and nothing else', () => {
  const original = { backup_enabled: true, close_to_tray: true, theme: 'dark', ollama_base_url: 'http://real:11434' };
  assert.equal(harnessConfig(original, '/v', 'id', '/b').backup_enabled, true);
  const next = harnessConfig(original, '/v', 'id', '/b', { driven: true });
  assert.equal(next.backup_enabled, false);
  assert.equal(next.close_to_tray, false);
  assert.equal(next.theme, 'dark');
  assert.equal(next.ollama_base_url, 'http://real:11434');
  assert.equal(harnessConfig(original, '/v', 'id', '/b', { ollamaBaseUrl: 'http://127.0.0.1:11435' }).ollama_base_url, 'http://127.0.0.1:11435');
});

test('snapshot refuses to overwrite existing evidence', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'alpha-harness-snapshot-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const vault = join(root, 'vault');
  const snapshot = join(root, 'snapshot');
  mkdirSync(vault);
  mkdirSync(snapshot);
  writeFileSync(join(vault, 'note.md'), 'new');
  writeFileSync(join(snapshot, 'note.md'), 'old');

  assert.throws(() => snapshotVault(vault, snapshot), /snapshot already exists/);
  assert.equal(readFileSync(join(snapshot, 'note.md'), 'utf8'), 'old');
});

test('restore rejects an invalid timeout before touching either machine', () => {
  const result = spawnSync(process.execPath, [
    fileURLToPath(new URL('../scripts/alpha-harness.mjs', import.meta.url)),
    'restore', '--timeout-minutes', 'not-a-number',
  ], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /timeout must be a positive number/);
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

// A minimal archive in the layout the app's exporter writes: local headers, then the directory.
function zipOf(members) {
  const locals = [];
  const directory = [];
  let offset = 0;
  for (const [name, text, method] of members) {
    const data = method === 8 ? deflateRawSync(Buffer.from(text)) : Buffer.from(text);
    const nameBytes = Buffer.from(name);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(method, 8);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt16LE(nameBytes.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(method, 10);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt32LE(offset, 42);
    locals.push(local, nameBytes, data);
    directory.push(central, nameBytes);
    offset += local.length + nameBytes.length + data.length;
  }
  const directoryBytes = Buffer.concat(directory);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(members.length, 10);
  end.writeUInt32LE(directoryBytes.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directoryBytes, end]);
}

test('diagnostic archive members are read back whether stored or deflated', () => {
  const entries = zipEntries(zipOf([['metadata.json', '{"product":"Second Brain"}', 0], ['logs/app.log', 'started\n'.repeat(50), 8]]));
  assert.deepEqual([...entries.keys()], ['metadata.json', 'logs/app.log']);
  assert.equal(entries.get('logs/app.log').toString(), 'started\n'.repeat(50));
  assert.throws(() => zipEntries(Buffer.from('not a zip')), /not a ZIP archive/);
});

test('diagnostic leak check names each planted value and the member that holds it', () => {
  const entries = zipEntries(zipOf([
    ['config.json', '{"openai_api_key":"[redacted]"}', 8],
    ['logs/app.log', 'saved Projects/Walkthrough capture.md', 8],
  ]));
  assert.deepEqual(diagnosticLeaks(entries, { secret: 'sk-planted', path: 'Projects/Walkthrough capture.md' }), [
    { member: 'logs/app.log', planted: 'path' },
  ]);
});

test('diagnostic leak check finds a Windows path escaped in JSON or written with forward slashes', () => {
  const vault = 'D:\\SecondBrainTest\\sb88\\runs\\r1\\windows\\vault';
  const entries = zipEntries(zipOf([
    ['config.json', `{"vaults":[{"path":${JSON.stringify(vault)}}]}`, 8],
    ['logs/app.log', 'opened D:/SecondBrainTest/sb88/runs/r1/windows/vault', 8],
    ['manifest.json', '{"vault":"[redacted]"}', 8],
  ]));
  assert.deepEqual(diagnosticLeaks(entries, { vault }), [
    { member: 'config.json', planted: 'vault' },
    { member: 'logs/app.log', planted: 'vault' },
  ]);
});

test('walkthrough refuses to start without its installer, RPM, or Notion workspace', () => {
  const script = fileURLToPath(new URL('../scripts/alpha-harness.mjs', import.meta.url));
  const env = { ...process.env, SECOND_BRAIN_NOTION_TOKEN: '', SECOND_BRAIN_NOTION_PAGE: '' };
  const noInstaller = spawnSync(process.execPath, [script, 'walkthrough'], { encoding: 'utf8', env });
  assert.equal(noInstaller.status, 1);
  assert.match(noInstaller.stderr, /--windows-installer/);
  const noRpm = spawnSync(process.execPath, [script, 'walkthrough', '--windows-installer', 'D:\\setup.exe'], { encoding: 'utf8', env });
  assert.equal(noRpm.status, 1);
  assert.match(noRpm.stderr, /--fedora-rpm/);
  const noNotion = spawnSync(process.execPath, [
    script, 'walkthrough', '--windows-installer', 'D:\\setup.exe', '--fedora-rpm', 'candidate.rpm',
  ], { encoding: 'utf8', env });
  assert.equal(noNotion.status, 1);
  assert.match(noNotion.stderr, /SECOND_BRAIN_NOTION_TOKEN/);
});
