#!/usr/bin/env node

import { createHash, randomUUID } from 'node:crypto';
import {
  appendFileSync,
  closeSync,
  copyFileSync,
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readlinkSync,
  readdirSync,
  renameSync,
  rmSync,
  rmdirSync,
  statfsSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { homedir } from 'node:os';
import { isIP } from 'node:net';
import { dirname, join, resolve, sep, win32 } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { check, fixtureNote, generate, NOTE_COUNT } from './sync-fixture.mjs';

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO = resolve(dirname(SCRIPT_PATH), '..');
const WINDOWS_TOOLS = win32.join('D:\\SecondBrainTest\\sb88', 'tools');
const WINDOWS_ROOT = 'D:\\SecondBrainTest\\sb88';
const WINDOWS_APP = 'D:\\SecondBrainTest\\app\\second-brain.exe';
const LINUX_APP = '/usr/bin/second-brain';
const MIN_FREE_BYTES = 5 * 1024 ** 3;
const DEFAULT_TIMEOUT_MS = 20 * 60_000;
const WINDOWS_TASK = 'SecondBrainAlphaHarness';
// Test-only tools installed under D:\SecondBrainTest; msedgedriver must match the WebView2 version.
const WINDOWS_DRIVER = 'D:\\SecondBrainTest\\sb88\\driver\\bin\\tauri-driver.exe';
const WINDOWS_NATIVE_DRIVER = 'D:\\SecondBrainTest\\sb88\\driver\\msedge\\msedgedriver.exe';
const WINDOWS_DRIVER_TASK = 'SecondBrainAlphaHarnessDriver';
const WINDOWS_DRIVER_PORT = 4444;
const WINDOWS_TUNNEL_PORT = 4446;
const LINUX_DRIVER_PORT = 4444;
// Explicit, so a stray listener on tauri-driver's default (4445) fails loudly instead of being proxied to.
const LINUX_NATIVE_PORT = 4447;
const APP_SOURCES = ['src', 'src-tauri', 'static', 'pnpm-lock.yaml', 'svelte.config.js', 'vite.config.ts'];
const POLL_MS = 1_000;
const DEVICE_ID = /^[A-Z2-7]{7}(?:-[A-Z2-7]{7}){7}$/;

function fail(message) {
  throw new Error(message);
}

function sleep(milliseconds) {
  return new Promise((accept) => setTimeout(accept, milliseconds));
}

function atomicWrite(path, data) {
  const temporary = `${path}.alpha-harness-${process.pid}`;
  writeFileSync(temporary, data, { mode: 0o600 });
  renameSync(temporary, path);
}

function runCommandSync(executable, args, options = {}) {
  const { accept = [0], ...spawnOptions } = options;
  const result = spawnSync(executable, args, {
    encoding: 'utf8',
    windowsHide: true,
    timeout: 30_000,
    killSignal: 'SIGKILL',
    ...spawnOptions,
  });
  if (result.error) throw result.error;
  if (!accept.includes(result.status)) {
    fail(
      `${executable} exited ${result.status}: ${(result.stderr || result.stdout).trim()}`,
    );
  }
  return { status: result.status, stdout: result.stdout.trim(), stderr: result.stderr.trim() };
}

function parseLastJson(text) {
  const lines = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    try {
      return JSON.parse(lines[index]);
    } catch {
      // PowerShell over SSH can add CLIXML/progress lines. Only worker JSON matters.
    }
  }
  fail(`command returned no JSON: ${text.slice(-500)}`);
}

function syncPort(vaultId) {
  let value = 0;
  for (const character of vaultId.replaceAll(/[^0-9a-f]/gi, '').slice(0, 4)) {
    value = (value * 16 + Number.parseInt(character, 16)) & 0xffff;
  }
  return 18_000 + (value % 10_000);
}

export function harnessConfig(config, vaultPath, vaultId, backupPath, gate = 'sync') {
  const next = structuredClone(config);
  next.vaults = [{ path: vaultPath, name: 'Alpha Harness', vault_id: vaultId }];
  next.active_vault = vaultPath;
  next.active_bookmark_id = null;
  next.backup_location = backupPath;
  next.backup_max_count = Math.max(10, Number(next.backup_max_count) || 0);
  // The restore gate restores the one backup it made; a scheduled backup would add a second.
  if (gate === 'restore') next.backup_enabled = false;
  return next;
}

export function pairedSyncthingConfig(config, { vaultId, vaultPath, peerId, peerName, peerIp, localIp }) {
  if (!DEVICE_ID.test(peerId)) fail(`invalid Syncthing device ID: ${peerId}`);
  const next = structuredClone(config);
  next.devices = [{
    deviceID: peerId,
    name: peerName,
    addresses: [`tcp://${peerIp}:22000`],
    autoAcceptFolders: false,
  }];
  next.folders = [{
    id: vaultId,
    label: 'Second Brain Vault',
    path: vaultPath,
    type: 'sendreceive',
    paused: true,
    devices: [{ deviceID: peerId }],
    fsWatcherEnabled: true,
  }];
  next.options ??= {};
  Object.assign(next.options, {
    globalAnnounceEnabled: false,
    localAnnounceEnabled: false,
    relaysEnabled: false,
    natEnabled: false,
    crashReportingEnabled: false,
    urAccepted: -1,
    listenAddresses: [`tcp://${localIp}:22000`],
  });
  return next;
}

function makeControl(vaultId) {
  return {
    version: 1,
    enabled: true,
    apiKey: randomUUID().replaceAll('-', ''),
    guiPort: syncPort(vaultId),
    peer: null,
  };
}

function requireFreeSpace(path) {
  const stats = statfsSync(path);
  const free = Number(stats.bavail) * Number(stats.bsize);
  if (free < MIN_FREE_BYTES) fail(`${path} has less than 5 GiB free`);
  return free;
}

function makeVault(vaultPath, vaultId) {
  mkdirSync(join(vaultPath, '.helixnotes'), { recursive: true });
  for (const category of ['Projects', 'Areas', 'Resources', 'Archives']) {
    mkdirSync(join(vaultPath, category));
  }
  writeFileSync(join(vaultPath, '.helixnotes', 'vault_id'), vaultId, { flag: 'wx' });
}

function localTemplateConfig() {
  const configHome = process.env.XDG_CONFIG_HOME || join(homedir(), '.config');
  const path = join(configHome, 'helixnotes', 'config.json');
  if (!existsSync(path)) fail(`installed app config is missing: ${path}`);
  return JSON.parse(readFileSync(path, 'utf8'));
}

function prepareLinux(root, runId, vaultId, gate = 'sync') {
  requireFreeSpace(root);
  const runRoot = join(root, 'runs', runId, 'fedora');
  if (existsSync(runRoot)) fail(`run already exists: ${runRoot}`);
  const vaultPath = join(runRoot, 'vault');
  const backupPath = join(runRoot, 'backups');
  const configHome = join(runRoot, 'xdg-config');
  const dataHome = join(runRoot, 'xdg-data');
  const configPath = join(configHome, 'helixnotes', 'config.json');
  const machinePath = join(dataHome, 'helixnotes', 'vaults', vaultId);
  mkdirSync(dirname(configPath), { recursive: true });
  mkdirSync(machinePath, { recursive: true });
  mkdirSync(backupPath, { recursive: true });
  makeVault(vaultPath, vaultId);
  // Without a control file sync stays off, so the restore gate runs with no sidecar.
  const control = gate === 'sync' ? makeControl(vaultId) : null;
  if (control) atomicWrite(join(machinePath, 'sync-control.json'), `${JSON.stringify(control, null, 2)}\n`);
  atomicWrite(
    configPath,
    `${JSON.stringify(harnessConfig(localTemplateConfig(), vaultPath, vaultId, backupPath, gate), null, 2)}\n`,
  );
  return { runRoot, vaultPath, backupPath, configHome, dataHome, machinePath, control };
}

function processRows() {
  const rows = [];
  for (const name of readdirSync('/proc').filter((entry) => /^\d+$/.test(entry))) {
    try {
      const root = join('/proc', name);
      const executable = readlinkSync(join(root, 'exe'));
      const commandLine = readFileSync(join(root, 'cmdline'), 'utf8').split('\0').filter(Boolean);
      const status = readFileSync(join(root, 'status'), 'utf8');
      const parentPid = Number(/^PPid:\s+(\d+)$/m.exec(status)?.[1] ?? 0);
      rows.push({ pid: Number(name), parentPid, executable, commandLine });
    } catch {
      // Process exited while /proc was read.
    }
  }
  return rows;
}

function linuxReport(machinePath) {
  return processRows()
    .filter((row) =>
      row.executable === LINUX_APP ||
      (row.executable === '/usr/bin/syncthing' && row.commandLine.some((arg) => arg.includes(machinePath)))
    )
    .map((row) => ({
      role: row.executable === '/usr/bin/syncthing'
        ? 'Sidecar'
        : row.commandLine.includes('--helix-sync-watchdog') ? 'Watchdog' : 'App',
      ...row,
    }));
}

function assertNoLinuxApp() {
  const apps = processRows().filter((row) => row.executable === LINUX_APP);
  if (apps.length) fail(`installed Fedora app is already running (pid ${apps[0].pid})`);
}

function launchLinux(machine, logName) {
  const log = openSync(join(machine.runRoot, logName), 'a');
  const child = spawn(LINUX_APP, [], {
    env: {
      ...process.env,
      XDG_CONFIG_HOME: machine.configHome,
      XDG_DATA_HOME: machine.dataHome,
    },
    stdio: ['ignore', log, log],
  });
  closeSync(log);
  if (!child.pid) fail('Fedora app did not return a pid');
  return child;
}

async function stopLinux(child, machine) {
  if (child?.exitCode === null) {
    child.kill('SIGTERM');
    const deadline = Date.now() + 15_000;
    while (child.exitCode === null && Date.now() < deadline) await sleep(250);
    if (child.exitCode === null) child.kill('SIGKILL');
  }
  const cleanupDeadline = Date.now() + 60_000;
  while (linuxReport(machine.machinePath).length && Date.now() < cleanupDeadline) await sleep(500);
  const survivors = linuxReport(machine.machinePath);
  if (survivors.length) fail(`Fedora process survived app exit: ${JSON.stringify(survivors)}`);
}

function base64Request(value) {
  return Buffer.from(JSON.stringify(value)).toString('base64url');
}

function remoteWorker(sshHost, request, timeout = 45_000) {
  const remoteScript = win32.join(WINDOWS_TOOLS, 'alpha-harness.mjs');
  const result = runCommandSync('ssh', [
    sshHost,
    'node',
    remoteScript,
    '__windows-worker',
    base64Request(request),
  ], { timeout });
  return parseLastJson(result.stdout);
}

function encodedPowerShell(script) {
  return Buffer.from(script, 'utf16le').toString('base64');
}

// PowerSetRequest rather than SetThreadExecutionState: `powercfg /requests` lists it by process
// and reason, so a run can be checked for its hold. Windows OpenSSH does not end a session's
// processes when the client disconnects, so the holder bounds itself with a deadline instead.
function windowsKeepAwake(holdMinutes) {
  return `
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace AlphaHarness {
  public static class Power {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct ReasonContext { public uint Version; public uint Flags; public string Reason; }
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr PowerCreateRequest(ref ReasonContext context);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool PowerSetRequest(IntPtr request, int requestType);
    public static void HoldSystem(string reason) {
      var context = new ReasonContext { Version = 0, Flags = 1, Reason = reason };
      var request = PowerCreateRequest(ref context);
      if (request == new IntPtr(-1) || !PowerSetRequest(request, 1)) throw new System.ComponentModel.Win32Exception();
    }
  }
}
'@
[AlphaHarness.Power]::HoldSystem('${WINDOWS_KEEP_AWAKE_REASON}')
"held $PID"
$holdDeadline = (Get-Date).AddMinutes(${holdMinutes})
while ((Get-Date) -lt $holdDeadline) { Start-Sleep -Seconds 30 }
`;
}

const WINDOWS_KEEP_AWAKE_REASON = 'Second Brain alpha harness run';

// Run 20260921T023725Z died when Windows slept (Kernel-Power 42) seven minutes in. Fedora is held
// by systemd-inhibit for the controller's lifetime; Windows by a power request whose holder the
// controller stops by its reported PID, and which expires by itself if the controller dies.
async function keepAwake(sshHost, timeoutMs) {
  const holdMinutes = Math.ceil((3 * timeoutMs) / 60_000) + 15;
  const fedora = spawn('systemd-inhibit', [
    '--what=sleep:idle', '--who=alpha-harness', `--why=${WINDOWS_KEEP_AWAKE_REASON}`, 'sleep', 'infinity',
  ], { stdio: 'ignore' });
  const windows = spawn('ssh', [
    sshHost, 'powershell.exe', '-NoProfile', '-NonInteractive', '-EncodedCommand',
    encodedPowerShell(windowsKeepAwake(holdMinutes)),
  ], { stdio: ['ignore', 'pipe', 'ignore'] });
  let holderPid;
  let lost;
  const release = () => {
    fedora.kill();
    windows.kill();
    if (holderPid) stopWindowsHolder(sshHost, holderPid);
  };
  try {
    await new Promise((resolveHeld, rejectHeld) => {
      const timer = setTimeout(() => rejectHeld(new Error('Windows keep-awake did not start within 60 s')), 60_000);
      let output = '';
      windows.stdout.on('data', (chunk) => {
        output += chunk;
        const held = /^held (\d+)\r?$/m.exec(output);
        if (held) { holderPid = Number(held[1]); clearTimeout(timer); resolveHeld(); }
      });
      windows.on('exit', () => { clearTimeout(timer); rejectHeld(new Error('Windows keep-awake exited before holding')); });
      fedora.on('exit', () => { clearTimeout(timer); rejectHeld(new Error('systemd-inhibit exited before the run')); });
    });
  } catch (error) {
    release();
    throw error;
  }
  // Losing a hold mid-run means the machine may sleep again; the run reports it instead of
  // passing quietly.
  fedora.on('exit', () => { lost ??= 'systemd-inhibit exited during the run'; });
  windows.on('exit', () => { lost ??= 'Windows keep-awake holder exited during the run'; });
  return { holdMinutes, release, lost: () => lost };
}

// AGENTS.md rule 3: the holder reported its own PID; confirm it is still that harness PowerShell
// before stopping it.
function stopWindowsHolder(sshHost, holderPid) {
  const script = `
$holderRecord = Get-CimInstance Win32_Process -Filter "ProcessId = ${holderPid}"
if ($holderRecord -and $holderRecord.Name -eq 'powershell.exe' -and $holderRecord.CommandLine -like '*-NonInteractive -EncodedCommand*') {
  Stop-Process -Id ${holderPid} -Force
}`;
  runCommandSync('ssh', [sshHost, 'powershell.exe', '-NoProfile', '-NonInteractive', '-EncodedCommand', encodedPowerShell(script)]);
}

function deployWindowsTools(sshHost) {
  runCommandSync('ssh', [
    sshHost,
    'powershell.exe',
    '-NoProfile',
    '-NonInteractive',
    '-EncodedCommand',
    encodedPowerShell(`New-Item -ItemType Directory -Force -Path '${WINDOWS_TOOLS.replaceAll("'", "''")}' | Out-Null`),
  ]);
  runCommandSync('scp', [
    join(REPO, 'scripts', 'alpha-harness.mjs'),
    join(REPO, 'scripts', 'sync-fixture.mjs'),
    join(REPO, 'scripts', 'windows', 'alpha-harness.ps1'),
    join(REPO, 'scripts', 'windows', 'sync-interrupt.ps1'),
    `${sshHost}:D:/SecondBrainTest/sb88/tools/`,
  ], { timeout: 60_000 });
}

async function syncthingRequest(control, method, path, body) {
  const response = await fetch(`http://127.0.0.1:${control.guiPort}${path}`, {
    method,
    headers: { 'X-API-Key': control.apiKey, ...(body ? { 'Content-Type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) fail(`Syncthing ${path} returned HTTP ${response.status}`);
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

async function waitForSidecar(control, tailscaleIp, machine, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const status = await syncthingRequest(control, 'GET', '/rest/system/status');
      return { deviceId: status.myID, tailscaleIp: tailscaleIp() };
    } catch (error) {
      lastError = error;
      await sleep(500);
    }
  }
  fail(`${machine} sidecar did not become ready: ${lastError?.message}`);
}

function parseTailscaleIp(output) {
  const address = output.split(/\s+/).find((value) => {
    if (isIP(value) !== 4) return false;
    const [first, second] = value.split('.').map(Number);
    return first === 100 && second >= 64 && second <= 127;
  });
  if (!address) fail('Tailscale did not report a 100.64.0.0/10 IPv4 address');
  return address;
}

function localTailscaleIp() {
  return parseTailscaleIp(runCommandSync('tailscale', ['ip', '-4']).stdout);
}

async function configureLocal(machine, peer) {
  const controlPath = join(machine.machinePath, 'sync-control.json');
  machine.control.peer = { deviceId: peer.deviceId, name: 'Windows', tailscaleIp: peer.tailscaleIp };
  atomicWrite(controlPath, `${JSON.stringify(machine.control, null, 2)}\n`);
  const current = await syncthingRequest(machine.control, 'GET', '/rest/config');
  const next = pairedSyncthingConfig(current, {
    vaultId: peer.vaultId,
    vaultPath: machine.vaultPath,
    peerId: peer.deviceId,
    peerName: 'Windows',
    peerIp: peer.tailscaleIp,
    localIp: localTailscaleIp(),
  });
  await syncthingRequest(machine.control, 'PUT', '/rest/config', next);
  return syncthingRequest(machine.control, 'GET', '/rest/config/restart-required');
}

// Traces are committed publicly: no account names, home paths, or host names (docs/log-privacy.md).
export function redactTrace(json) {
  return json
    .replaceAll(JSON.stringify(homedir()).slice(1, -1), '~')
    .replace(/C:\\\\Users\\\\[^\\\\"]+/gi, '%USERPROFILE%')
    .replace(/,"host":"[^"]*"|"host":"[^"]*",?/g, '');
}

function observation(writer, event, machine, value = {}) {
  const line = { at: new Date().toISOString(), event, machine, ...value };
  appendFileSync(writer, `${redactTrace(JSON.stringify(line))}\n`, { flag: 'a' });
  return line;
}

async function pollWindowsFixture({ sshHost, runId, evidencePath, predicate, deadline }) {
  while (Date.now() < deadline) {
    const result = remoteWorker(sshHost, { action: 'check', root: WINDOWS_ROOT, runId });
    observation(evidencePath, 'fixture-poll', 'windows', result.observation);
    if (predicate(result.observation)) return result.observation;
    await sleep(POLL_MS);
  }
  fail('fixture polling timed out');
}

async function waitForWindowsProcesses(sshHost, runId, predicate, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  let report;
  while (Date.now() < deadline) {
    report = remoteWorker(sshHost, { action: 'report', root: WINDOWS_ROOT, runId });
    if (predicate(report.processes)) return report;
    await sleep(500);
  }
  fail(`Windows process state timed out: ${JSON.stringify(report)}`);
}

// Gate 2: interrupted restore. The restore journal and its stage and rollback directories sit
// next to the vault (src-tauri/src/backup.rs, #142); their presence after a kill is the proof
// that the kill landed inside a restore.
const RESTORE_PREFIX = '.second-brain-restore-';
export const KILL_POINTS = { journal: 0, 'stage-half': 0.5, 'stage-late': 0.95 };

function byName(left, right) {
  return left.name < right.name ? -1 : left.name > right.name ? 1 : 0;
}

// Every directory and file under the vault, `.helixnotes` included: a recovered vault must equal
// the pre-restore or the restored state byte for byte, metadata and all.
export function treeHash(root) {
  const lines = [];
  let files = 0;
  let directories = 0;
  const visit = (directory, prefix) => {
    for (const entry of readdirSync(directory, { withFileTypes: true }).sort(byName)) {
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        directories += 1;
        lines.push(`d ${relative}`);
        visit(path, relative);
      } else if (entry.isFile()) {
        files += 1;
        lines.push(`f ${relative} ${createHash('sha256').update(readFileSync(path)).digest('hex')}`);
      } else {
        fail(`unexpected non-file entry in vault: ${relative}`);
      }
    }
  };
  visit(root, '');
  return { sha256: createHash('sha256').update(lines.join('\n')).digest('hex'), files, directories };
}

function countFiles(directory) {
  return readdirSync(directory, { recursive: true, withFileTypes: true }).filter((entry) => entry.isFile()).length;
}

export function restoreProgress(vault) {
  const parent = dirname(vault);
  const leftovers = readdirSync(parent).filter((name) => name.startsWith(RESTORE_PREFIX)).sort();
  const journal = leftovers.find((name) => name.startsWith(`${RESTORE_PREFIX}journal-`) && name.endsWith('.json'));
  const stage = leftovers.find((name) => name.startsWith(`${RESTORE_PREFIX}stage-`));
  let phase = null;
  if (journal) {
    try {
      phase = JSON.parse(readFileSync(join(parent, journal), 'utf8')).phase;
    } catch {
      phase = 'unreadable';
    }
  }
  let stagedFiles = 0;
  if (stage) {
    try {
      stagedFiles = countFiles(join(parent, stage));
    } catch {
      // The commit moves staged entries out while this counts them.
      stagedFiles = -1;
    }
  }
  return {
    leftovers,
    phase,
    stagedFiles,
    rollback: leftovers.some((name) => name.startsWith(`${RESTORE_PREFIX}rollback-`)),
  };
}

export function killDue(point, progress, expectedFiles) {
  if (!(point in KILL_POINTS)) fail(`unknown kill point: ${point}`);
  return progress.phase !== null && progress.stagedFiles >= KILL_POINTS[point] * expectedFiles;
}

// State B, the pre-restore vault: differs from the backup (state A) by edited, deleted, and added
// notes, so undo and publish are told apart by hash.
export function mutateFixture(vault) {
  let edited = 0;
  let removed = 0;
  for (let index = 0; index < NOTE_COUNT; index += 1) {
    const path = join(vault, fixtureNote(index).path);
    if (index % 10 === 0) {
      appendFileSync(path, 'Edited after the backup.\n');
      edited += 1;
    } else if (index % 10 === 1) {
      unlinkSync(path);
      removed += 1;
    }
  }
  const added = 100;
  for (let index = 0; index < added; index += 1) {
    writeFileSync(join(vault, 'Projects', `After backup ${index}.md`), `Written after the backup, ${index}.\n`, { flag: 'wx' });
  }
  return { edited, removed, added };
}

function processAlive(pid) {
  // A killed child stays a zombie until its parent reaps it; it is no longer running.
  if (process.platform === 'linux') {
    try {
      return !/^\d+ \(.*\) Z/s.test(readFileSync(`/proc/${pid}/stat`, 'utf8'));
    } catch {
      return false;
    }
  }
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === 'EPERM';
  }
}

// Runs next to the app (locally on Fedora, over SSH on Windows) because a controller round trip
// per poll is slower than the whole unpack. The caller has proven `pid` is this run's app.
async function killWatch({ vault, point, expectedFiles, pid, timeoutMs }) {
  console.log(JSON.stringify({ watching: true }));
  const deadline = Date.now() + timeoutMs;
  let journalSeen = false;
  for (;;) {
    const progress = restoreProgress(vault);
    if (progress.phase !== null) journalSeen = true;
    else if (journalSeen) return { point, missed: true, progress };
    if (killDue(point, progress, expectedFiles)) {
      process.kill(pid, 'SIGKILL');
      const killedAt = new Date().toISOString();
      const exitDeadline = Date.now() + 15_000;
      while (processAlive(pid) && Date.now() < exitDeadline) await sleep(50);
      if (processAlive(pid)) fail(`app ${pid} survived the kill`);
      return { point, pid, killedAt, atKill: progress, afterKill: restoreProgress(vault) };
    }
    if (Date.now() > deadline) fail(`restore did not reach kill point ${point}: ${JSON.stringify(progress)}`);
    await sleep(2);
  }
}

function startKillWatch(executable, args) {
  const child = spawn(executable, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '';
  let errors = '';
  let markReady;
  const ready = new Promise((accept) => { markReady = accept; });
  child.stdout.on('data', (chunk) => {
    output += chunk;
    if (output.includes('"watching":true')) markReady();
  });
  child.stderr.on('data', (chunk) => { errors += chunk; });
  const result = new Promise((accept, reject) => {
    child.on('exit', (code) => {
      markReady();
      if (code === 0) accept(parseLastJson(output));
      else reject(new Error(`kill watch exited ${code}: ${(errors || output).trim().slice(-500)}`));
    });
  });
  return { ready, result };
}

function resetVault(vault, snapshot, root) {
  for (const path of [vault, snapshot]) {
    if (!resolve(path).startsWith(`${resolve(root)}${sep}`)) fail(`path leaves run root: ${path}`);
  }
  rmSync(vault, { recursive: true, force: true });
  cpSync(snapshot, vault, { recursive: true });
}

async function waitForDriver(port, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      await fetch(`http://127.0.0.1:${port}/status`, { signal: AbortSignal.timeout(2_000) });
      return;
    } catch (error) {
      lastError = error;
      await sleep(500);
    }
  }
  fail(`WebDriver on port ${port} did not answer: ${lastError?.message}`);
}

async function openApp(port, application) {
  // Only the controller needs webdriverio; the Windows worker copy runs without node_modules.
  const { remote } = await import('webdriverio');
  const browser = await remote({
    hostname: '127.0.0.1',
    port,
    logLevel: 'warn',
    connectionRetryCount: 0,
    capabilities: { 'tauri:options': { application } },
  });
  await browser.$('button=New Note').waitForDisplayed({ timeout: 60_000 });
  return browser;
}

async function closeApp(browser) {
  try {
    await browser.deleteSession();
    return null;
  } catch (error) {
    // Expected after a kill: the session's app is already gone.
    return error.message;
  }
}

// WebKitWebDriver answers element-click with "unsupported operation", so every click is a DOM
// click; the app's handlers are plain onclick, which it triggers the same way.
async function press(browser, pending) {
  const element = await pending;
  await element.waitForDisplayed({ timeout: 30_000 });
  // A DOM click on a disabled button is silently ignored.
  await element.waitForEnabled({ timeout: 30_000 });
  await browser.execute((target) => target.click(), element);
}

async function openBackupTab(browser) {
  await press(browser, browser.$('button[title="Settings"]'));
  await press(browser, browser.$('button.tab-btn=Backup'));
}

async function backupThroughUi(browser) {
  await openBackupTab(browser);
  await press(browser, browser.$('button.backup-link-btn*=Backup now'));
  const message = browser.$('.import-result');
  await message.waitForDisplayed({ timeout: 5 * 60_000 });
  const text = await message.getText();
  if (!text.includes('Backup created successfully')) fail(`backup failed in the app: ${text}`);
}

// Leaves the confirmation's Restore button ready so the caller can arm its watcher first.
async function openRestoreConfirmation(browser) {
  await openBackupTab(browser);
  const restoreButtons = await browser.$$('button.backup-action-btn[title="Restore"]');
  if (restoreButtons.length !== 1) fail(`expected exactly one backup to restore, found ${restoreButtons.length}`);
  await press(browser, restoreButtons[0]);
  const confirm = browser.$('button.restore-confirm-btn');
  await confirm.waitForDisplayed({ timeout: 10_000 });
  return confirm;
}

// Repair status loads after the note list renders, so the banner can arrive a moment later.
// Dismissing it afterwards means the next trial's notice cannot be this one still showing.
async function readAndDismissNotice(browser) {
  const banner = browser.$('.repair-banner');
  try {
    await banner.waitForDisplayed({ timeout: 20_000 });
  } catch {
    return null;
  }
  const notice = { title: await banner.$('strong').getText(), message: await banner.$('span').getText() };
  const started = Date.now();
  await press(browser, banner.$('button=Dismiss'));
  await banner.waitForExist({ reverse: true, timeout: 60_000 });
  return { ...notice, dismissedMs: Date.now() - started };
}

function parseOptions(args) {
  const options = {
    sshHost: 'sb-windows',
    linuxRoot: join(homedir(), 'sb88'),
    timeoutMs: DEFAULT_TIMEOUT_MS,
    candidate: 'HEAD',
  };
  for (let index = 0; index < args.length; index += 2) {
    const name = args[index];
    const value = args[index + 1];
    if (!value) fail(`missing value for ${name}`);
    if (name === '--ssh') options.sshHost = value;
    else if (name === '--linux-root') options.linuxRoot = resolve(value);
    else if (name === '--candidate') options.candidate = value;
    else if (name === '--timeout-minutes') options.timeoutMs = Number(value) * 60_000;
    else fail(`unknown option: ${name}`);
  }
  return options;
}

function controllerIsRunning(pid) {
  if (pid === process.pid) return true;
  try {
    const commandLine = readFileSync(`/proc/${pid}/cmdline`, 'utf8');
    return commandLine.includes('alpha-harness.mjs');
  } catch {
    return false;
  }
}

export function acquireControllerLock(root) {
  const path = join(root, 'alpha-harness.lock.json');
  for (;;) {
    const token = randomUUID();
    try {
      writeFileSync(path, `${JSON.stringify({ pid: process.pid, token, startedAt: new Date().toISOString() })}\n`, {
        flag: 'wx', mode: 0o600,
      });
      return () => {
        const current = JSON.parse(readFileSync(path, 'utf8'));
        if (current.token !== token) fail('controller lock changed ownership during the run');
        unlinkSync(path);
      };
    } catch (error) {
      if (error.code !== 'EEXIST') throw error;
      const owner = JSON.parse(readFileSync(path, 'utf8'));
      if (controllerIsRunning(owner.pid)) fail(`another alpha harness owns the machines (pid ${owner.pid})`);
      const stale = `${path}.stale-${randomUUID()}`;
      try {
        renameSync(path, stale);
        unlinkSync(stale);
      } catch (renameError) {
        if (renameError.code !== 'ENOENT') throw renameError;
      }
    }
  }
}

function binaryEvidence(path, candidateCommit) {
  const bytes = readFileSync(path);
  if (!bytes.includes(Buffer.from(candidateCommit))) {
    fail(`${path} was not built from candidate ${candidateCommit}`);
  }
  return {
    path,
    bytes: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex'),
    buildCommit: candidateCommit,
  };
}

function fedoraPackageEvidence(candidateCommit) {
  const release = Object.fromEntries(
    readFileSync('/etc/os-release', 'utf8')
      .split('\n')
      .filter((line) => line.includes('='))
      .map((line) => {
        const index = line.indexOf('=');
        return [line.slice(0, index), line.slice(index + 1).replace(/^"|"$/g, '')];
      }),
  );
  if (release.ID !== 'fedora' || release.VERSION_ID !== '44' || process.arch !== 'x64') {
    fail(`unsupported Fedora target: ${release.ID} ${release.VERSION_ID} ${process.arch}`);
  }
  return {
    os: `${release.NAME} ${release.VERSION_ID}`,
    arch: process.arch,
    package: runCommandSync('rpm', ['-q', '--qf', '%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}', 'second-brain']).stdout,
    app: binaryEvidence(LINUX_APP, candidateCommit),
  };
}

async function runSync(args) {
  const options = parseOptions(args);
  if (process.platform !== 'linux') fail('sync controller must run on Fedora');
  if (!Number.isFinite(options.timeoutMs) || options.timeoutMs <= 0) fail('timeout must be a positive number');
  mkdirSync(options.linuxRoot, { recursive: true });
  const lockRoot = join(homedir(), '.cache', 'second-brain');
  mkdirSync(lockRoot, { recursive: true });
  const releaseLock = acquireControllerLock(lockRoot);
  let awake;
  try {
    awake = await keepAwake(options.sshHost, options.timeoutMs);
    const result = await runSyncLocked({ ...options, holdMinutes: awake.holdMinutes });
    if (awake.lost()) fail(`${awake.lost()}; the result cannot be trusted`);
    return result;
  } finally {
    awake?.release();
    releaseLock();
  }
}

async function runSyncLocked(options) {
  if (!existsSync(LINUX_APP)) fail(`installed app missing: ${LINUX_APP}`);
  assertNoLinuxApp();
  requireFreeSpace(options.linuxRoot);
  const candidateCommit = runCommandSync('git', ['rev-parse', '--verify', `${options.candidate}^{commit}`], { cwd: REPO }).stdout;
  const harnessCommit = runCommandSync('git', ['rev-parse', 'HEAD'], { cwd: REPO }).stdout;
  // Harness-only commits reuse the candidate's packages, so the app itself must be identical.
  const drift = runCommandSync('git', ['diff', '--name-only', candidateCommit, harnessCommit, '--', ...APP_SOURCES], { cwd: REPO }).stdout;
  if (drift) fail(`app sources differ from candidate ${candidateCommit}:\n${drift}`);
  const fedoraPackage = fedoraPackageEvidence(candidateCommit);
  deployWindowsTools(options.sshHost);
  const recovered = remoteWorker(options.sshHost, {
    action: 'recover', root: WINDOWS_ROOT, runId: 'recovery', appPath: WINDOWS_APP,
  });

  const runId = new Date().toISOString().replaceAll(/[-:]/g, '').replace(/\.\d{3}Z$/, 'Z');
  const vaultId = randomUUID();
  const evidencePath = join(REPO, 'docs', 'reports', 'evidence', `alpha-harness-sync-${runId}.jsonl`);
  mkdirSync(dirname(evidencePath), { recursive: true });
  const linux = prepareLinux(options.linuxRoot, runId, vaultId);
  let windowsPrepared = false;
  let linuxChild;
  let windowsPid;
  let completed = false;

  observation(evidencePath, 'run-start', 'controller', {
    runId,
    vaultId,
    commit: candidateCommit,
    harnessCommit,
  });
  observation(evidencePath, 'package-evidence', 'fedora', fedoraPackage);
  observation(evidencePath, 'keep-awake', 'controller', {
    fedora: 'systemd-inhibit sleep:idle',
    windows: `PowerSetRequest SystemRequired, ${options.holdMinutes} min`,
  });
  if (recovered.recovered) observation(evidencePath, 'stale-run-recovered', 'windows', recovered);

  let runError;
  let cleanupError;
  try {
    windowsPrepared = true;
    const prepared = remoteWorker(options.sshHost, {
      action: 'prepare', root: WINDOWS_ROOT, runId, vaultId, appPath: WINDOWS_APP,
      candidateCommit,
    });
    observation(evidencePath, 'prepared', 'windows', prepared);

    const generated = generate(linux.vaultPath);
    observation(evidencePath, 'fixture-generated', 'fedora', generated);
    const initial = remoteWorker(options.sshHost, { action: 'check', root: WINDOWS_ROOT, runId }).observation;
    observation(evidencePath, 'fixture-poll', 'windows', initial);
    if (initial.matching !== 0 || initial.missing !== NOTE_COUNT) {
      fail(`receiver is not empty: ${JSON.stringify(initial)}`);
    }

    linuxChild = launchLinux(linux, 'app-first.log');
    const launched = remoteWorker(options.sshHost, { action: 'launch', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP });
    windowsPid = launched.pid;
    observation(evidencePath, 'app-launched', 'fedora', { pid: linuxChild.pid });
    observation(evidencePath, 'app-launched', 'windows', launched);

    const [linuxIdentity, windowsIdentity] = await Promise.all([
      waitForSidecar(linux.control, localTailscaleIp, 'Fedora'),
      Promise.resolve(remoteWorker(options.sshHost, { action: 'identity', root: WINDOWS_ROOT, runId })),
    ]);
    if (!DEVICE_ID.test(linuxIdentity.deviceId) || !DEVICE_ID.test(windowsIdentity.deviceId)) {
      fail('Syncthing returned an invalid device ID');
    }
    await configureLocal(linux, { ...windowsIdentity, vaultId });
    remoteWorker(options.sshHost, {
      action: 'configure', root: WINDOWS_ROOT, runId,
      peerId: linuxIdentity.deviceId, peerIp: linuxIdentity.tailscaleIp, peerName: 'Fedora',
    });
    observation(evidencePath, 'paired', 'controller', {
      fedoraDevice: 'device-1',
      windowsDevice: 'device-2',
    });

    await stopLinux(linuxChild, linux);
    linuxChild = undefined;
    remoteWorker(options.sshHost, { action: 'stop-app', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP, pid: windowsPid });
    windowsPid = undefined;
    await waitForWindowsProcesses(options.sshHost, runId, (processes) => processes.length === 0, 60_000);

    linuxChild = launchLinux(linux, 'app-transfer.log');
    windowsPid = remoteWorker(options.sshHost, { action: 'launch', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP }).pid;
    const partial = await pollWindowsFixture({
      sshHost: options.sshHost,
      runId,
      evidencePath,
      deadline: Date.now() + options.timeoutMs,
      predicate: (value) => {
        if (value.complete) fail('transfer completed before interruption could be proven');
        return value.matching > 0;
      },
    });
    observation(evidencePath, 'mid-transfer-proven', 'windows', { matching: partial.matching, missing: partial.missing });

    const beforeStop = remoteWorker(options.sshHost, { action: 'report', root: WINDOWS_ROOT, runId });
    const oldSidecar = beforeStop.processes.find((process) => process.Role === 'Sidecar');
    if (!oldSidecar) fail('Windows sidecar was not running at interruption point');
    const interrupted = remoteWorker(options.sshHost, { action: 'interrupt-sidecar', root: WINDOWS_ROOT, runId });
    observation(evidencePath, 'sidecar-stopped', 'windows', interrupted);
    if (interrupted.pid !== oldSidecar.Id) fail('interrupt stopped an untracked sidecar pid');
    const afterStop = remoteWorker(options.sshHost, { action: 'check', root: WINDOWS_ROOT, runId }).observation;
    observation(evidencePath, 'fixture-poll', 'windows', afterStop);
    if (afterStop.complete) fail('transfer completed before interruption could be proven');

    const restarted = await waitForWindowsProcesses(
      options.sshHost,
      runId,
      (processes) => processes.some((process) => process.Role === 'Sidecar' && process.Id !== oldSidecar.Id),
    );
    observation(evidencePath, 'sidecar-relaunched', 'windows', {
      pid: restarted.processes.find((process) => process.Role === 'Sidecar' && process.Id !== oldSidecar.Id)?.Id,
    });

    await stopLinux(linuxChild, linux);
    linuxChild = undefined;
    remoteWorker(options.sshHost, { action: 'stop-app', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP, pid: windowsPid });
    windowsPid = undefined;
    await waitForWindowsProcesses(options.sshHost, runId, (processes) => processes.length === 0, 60_000);

    linuxChild = launchLinux(linux, 'app-recovery.log');
    windowsPid = remoteWorker(options.sshHost, { action: 'launch', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP }).pid;
    const finalWindows = await pollWindowsFixture({
      sshHost: options.sshHost,
      runId,
      evidencePath,
      deadline: Date.now() + options.timeoutMs,
      predicate: (value) => value.complete,
    });
    const finalFedora = check(linux.vaultPath);
    observation(evidencePath, 'fixture-final', 'fedora', finalFedora);
    if (!finalFedora.complete || !finalWindows.complete) fail('both vaults did not converge');

    const linuxBackups = readdirSync(linux.backupPath).filter((name) => name.startsWith('helixnotes-pre-sync-'));
    const windowsBackups = remoteWorker(options.sshHost, { action: 'backups', root: WINDOWS_ROOT, runId }).backups;
    if (!linuxBackups.length || !windowsBackups.length) fail('pre-sync backup missing on one machine');
    observation(evidencePath, 'pre-sync-backups', 'controller', { fedora: linuxBackups, windows: windowsBackups });
    completed = true;
  } catch (error) {
    runError = error;
    observation(evidencePath, 'run-failed', 'controller', { error: error.message });
  } finally {
    try {
      await stopLinux(linuxChild, linux);
      observation(evidencePath, 'process-final', 'fedora', { processes: linuxReport(linux.machinePath) });
    } catch (error) {
      observation(evidencePath, 'cleanup-error', 'fedora', { error: error.message });
      cleanupError ??= error;
    }
    if (windowsPrepared) {
      if (windowsPid) {
        try {
          remoteWorker(options.sshHost, { action: 'stop-app', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP, pid: windowsPid });
        } catch (error) {
          observation(evidencePath, 'cleanup-error', 'windows-app', { error: error.message });
          cleanupError ??= error;
        }
      }
      const finalizeError = await finalizeWindows(options.sshHost, runId, evidencePath);
      cleanupError ??= finalizeError;
    }
  }

  if (runError) throw runError;
  if (cleanupError) throw cleanupError;
  if (!completed) fail(`run failed; evidence: ${evidencePath}`);
  observation(evidencePath, 'run-complete', 'controller');
  console.log(JSON.stringify({ runId, evidencePath }));
}

function descendsFrom(rows, pid, ancestor) {
  const parents = new Map(rows.map((row) => [row.pid, row.parentPid]));
  for (let current = parents.get(pid); current; current = parents.get(current)) {
    if (current === ancestor) return true;
  }
  return false;
}

function linuxRestoreMachine(root, runId, vaultId) {
  const machine = prepareLinux(root, runId, vaultId, 'restore');
  const snapshotPath = join(machine.runRoot, 'vault-pre');
  let driver;
  const apps = () => {
    const rows = processRows();
    return rows.filter((row) => row.executable === LINUX_APP && driver && descendsFrom(rows, row.pid, driver.pid));
  };
  return {
    name: 'fedora',
    application: LINUX_APP,
    port: LINUX_DRIVER_PORT,
    prepare: () => ({ runRoot: machine.runRoot, vault: machine.vaultPath }),
    generate: () => generate(machine.vaultPath),
    hash: () => treeHash(machine.vaultPath),
    progress: () => restoreProgress(machine.vaultPath),
    mutate: () => mutateFixture(machine.vaultPath),
    backups: () => readdirSync(machine.backupPath),
    snapshot: () => cpSync(machine.vaultPath, snapshotPath, { recursive: true, errorOnExist: true }),
    reset: () => resetVault(machine.vaultPath, snapshotPath, machine.runRoot),
    async startDriver() {
      const log = openSync(join(machine.runRoot, 'tauri-driver.log'), 'a');
      driver = spawn('tauri-driver', ['--port', String(LINUX_DRIVER_PORT), '--native-port', String(LINUX_NATIVE_PORT)], {
        env: { ...process.env, XDG_CONFIG_HOME: machine.configHome, XDG_DATA_HOME: machine.dataHome },
        stdio: ['ignore', log, log],
      });
      closeSync(log);
      await waitForDriver(LINUX_DRIVER_PORT);
      // Something else answering on the port would pass the probe; the driver must still be up.
      if (driver.exitCode !== null) fail(`tauri-driver exited ${driver.exitCode}; is port ${LINUX_DRIVER_PORT} taken?`);
      return { pid: driver.pid };
    },
    appPid() {
      const found = apps().filter((row) => !row.commandLine.includes('--helix-sync-watchdog'));
      if (found.length !== 1) fail(`expected one app under tauri-driver ${driver?.pid}: ${JSON.stringify(found)}`);
      return found[0].pid;
    },
    killWatch(request) {
      return startKillWatch(process.execPath, [
        SCRIPT_PATH, '__kill-watch', base64Request({ ...request, vault: machine.vaultPath }),
      ]);
    },
    async appsGone() {
      const deadline = Date.now() + 60_000;
      while (linuxReport(machine.machinePath).length && Date.now() < deadline) await sleep(500);
      const survivors = linuxReport(machine.machinePath);
      if (survivors.length) fail(`Fedora process survived app exit: ${JSON.stringify(survivors)}`);
      return { processes: survivors };
    },
    async cleanup() {
      if (!driver) return this.appsGone();
      // Exact PIDs only: apps under this run's driver, then the driver's own children.
      for (const row of apps()) process.kill(row.pid, 'SIGKILL');
      for (const row of processRows().filter((candidate) => candidate.parentPid === driver.pid)) {
        process.kill(row.pid, 'SIGTERM');
      }
      if (driver.exitCode === null) {
        driver.kill('SIGTERM');
        const deadline = Date.now() + 15_000;
        while (driver.exitCode === null && Date.now() < deadline) await sleep(250);
      }
      return this.appsGone();
    },
  };
}

function windowsRestoreMachine(sshHost, runId, vaultId, candidateCommit) {
  const call = (action, extra = {}, timeout = 45_000) => remoteWorker(
    sshHost, { action, root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP, ...extra }, timeout,
  );
  const slow = 5 * 60_000;
  let driverPid;
  let tunnel;
  return {
    name: 'windows',
    application: WINDOWS_APP,
    port: WINDOWS_TUNNEL_PORT,
    prepare: () => call('prepare', { vaultId, candidateCommit, gate: 'restore' }),
    generate: () => call('generate', {}, slow),
    hash: () => call('hash', {}, slow),
    progress: () => call('progress'),
    mutate: () => call('mutate', {}, slow),
    backups: () => call('all-backups').backups,
    snapshot: () => call('snapshot', {}, slow),
    reset: () => call('reset', {}, slow),
    async startDriver() {
      const started = call('start-driver');
      driverPid = started.pid;
      tunnel = spawn('ssh', [
        '-N', '-o', 'ExitOnForwardFailure=yes',
        '-L', `127.0.0.1:${WINDOWS_TUNNEL_PORT}:127.0.0.1:${WINDOWS_DRIVER_PORT}`, sshHost,
      ], { stdio: 'ignore' });
      await waitForDriver(WINDOWS_TUNNEL_PORT);
      if (tunnel.exitCode !== null) fail(`WebDriver tunnel exited ${tunnel.exitCode}; is port ${WINDOWS_TUNNEL_PORT} taken?`);
      return started;
    },
    appPid: () => call('app-pid', { driverPid }).pid,
    killWatch(request) {
      return startKillWatch('ssh', [
        sshHost, 'node', win32.join(WINDOWS_TOOLS, 'alpha-harness.mjs'), '__kill-watch',
        base64Request({ ...request, vault: windowsPaths(WINDOWS_ROOT, runId).vaultPath }),
      ]);
    },
    appsGone: () => waitForWindowsProcesses(sshHost, runId, (processes) => processes.length === 0, 60_000),
    async cleanup(evidencePath) {
      let firstError;
      tunnel?.kill();
      if (driverPid) {
        try {
          observation(evidencePath, 'driver-stopped', 'windows', call('stop-driver', { driverPid }));
        } catch (error) {
          observation(evidencePath, 'cleanup-error', 'windows-driver', { error: error.message });
          firstError = error;
        }
      }
      firstError ??= await finalizeWindows(sshHost, runId, evidencePath);
      if (firstError) throw firstError;
    },
  };
}

async function waitForRestore(browser) {
  const message = browser.$('.import-result');
  await message.waitUntil(async () => /restored|failed/i.test(await message.getText().catch(() => '')), {
    timeout: 5 * 60_000,
    timeoutMsg: 'restore did not report a result',
  });
  const text = await message.getText();
  if (text !== 'Backup restored.') fail(`restore did not succeed in the app: ${text}`);
}

async function withApp(machine, action) {
  const browser = await openApp(machine.port, machine.application);
  try {
    return await action(browser);
  } finally {
    await closeApp(browser);
    await machine.appsGone();
  }
}

// A launch can still write to the vault: it normalizes notes it has not seen, and the next open
// sweeps the empty `.helixnotes/staging` that leaves behind. A state is only comparable once a
// further launch no longer changes it.
async function settle(machine, hash = machine.hash()) {
  for (let launch = 0; launch < 4; launch += 1) {
    await withApp(machine, async () => {});
    const next = machine.hash();
    if (next.sha256 === hash.sha256) return next;
    hash = next;
  }
  fail(`vault still changes on every launch: ${JSON.stringify(hash)}`);
}

export function recoveryOutcome(hash, pre, post) {
  if (hash === pre) return 'pre-state';
  if (hash === post) return 'post-state';
  return null;
}

const RECOVERY_NOTICE = {
  'pre-state': 'back exactly as it was before the restore',
  'post-state': 'kept as restored',
};

async function restoreGate(machine, evidencePath, screenshotDir) {
  const note = (event, value) => observation(evidencePath, event, machine.name, value);
  note('fixture-generated', await machine.generate());
  note('driver-started', await machine.startDriver());

  await withApp(machine, backupThroughUi);
  const backups = machine.backups();
  if (backups.length !== 1) fail(`expected one backup, found ${JSON.stringify(backups)}`);
  const backedUp = machine.hash();
  note('backup-created', { backups, vault: backedUp });

  note('vault-mutated', machine.mutate());
  const pre = await settle(machine);
  machine.snapshot();
  note('pre-state', pre);

  await withApp(machine, async (browser) => {
    await press(browser, await openRestoreConfirmation(browser));
    await waitForRestore(browser);
  });
  const post = await settle(machine);
  const afterControl = machine.progress();
  note('post-state', { vault: post, leftovers: afterControl.leftovers });
  if (post.sha256 === pre.sha256) fail('the restore did not change the vault');
  if (afterControl.leftovers.length) fail(`completed restore left files behind: ${afterControl.leftovers}`);

  for (const point of Object.keys(KILL_POINTS)) {
    machine.reset();
    if (machine.hash().sha256 !== pre.sha256) fail('vault reset did not reproduce the pre-state');
    const browser = await openApp(machine.port, machine.application);
    let killed;
    try {
      const confirm = await openRestoreConfirmation(browser);
      const pid = machine.appPid();
      const watch = machine.killWatch({ point, expectedFiles: backedUp.files, pid, timeoutMs: 5 * 60_000 });
      await watch.ready;
      await press(browser, confirm);
      killed = await watch.result;
    } finally {
      await closeApp(browser);
      await machine.appsGone();
    }
    note('app-killed', killed);
    // A journal that outlives the app is the proof the kill landed inside the restore.
    if (killed.missed || killed.afterKill.phase === null) fail(`kill at ${point} landed after the restore finished`);
    note('interrupted-state', { point, vault: machine.hash(), progress: machine.progress() });

    const banner = await withApp(machine, async (relaunched) => {
      await relaunched.$('.repair-banner').waitForDisplayed({ timeout: 20_000 }).catch(() => {});
      await relaunched.saveScreenshot(join(screenshotDir, `${machine.name}-${point}.png`));
      return readAndDismissNotice(relaunched);
    });
    const after = machine.progress();
    // Recorded before settling, so the evidence shows what recovery alone produced.
    const immediate = machine.hash();
    const recovered = await settle(machine, immediate);
    const outcome = recoveryOutcome(recovered.sha256, pre.sha256, post.sha256);
    note('recovered', { point, outcome, immediate, vault: recovered, leftovers: after.leftovers, banner });
    if (!outcome) fail(`vault after recovery from ${point} is neither the pre-restore nor the restored state`);
    if (after.leftovers.length) fail(`recovery from ${point} left files behind: ${after.leftovers}`);
    if (banner?.title !== 'Restore interrupted' || !banner.message.includes(RECOVERY_NOTICE[outcome])) {
      fail(`recovery notice after ${point} is missing or wrong: ${JSON.stringify(banner)}`);
    }
  }
}

async function runRestore(args) {
  const options = parseOptions(args);
  if (process.platform !== 'linux') fail('restore controller must run on Fedora');
  mkdirSync(options.linuxRoot, { recursive: true });
  const lockRoot = join(homedir(), '.cache', 'second-brain');
  mkdirSync(lockRoot, { recursive: true });
  const releaseLock = acquireControllerLock(lockRoot);
  let awake;
  try {
    awake = await keepAwake(options.sshHost, options.timeoutMs);
    await runRestoreLocked({ ...options, holdMinutes: awake.holdMinutes });
    if (awake.lost()) fail(`${awake.lost()}; the result cannot be trusted`);
  } finally {
    awake?.release();
    releaseLock();
  }
}

async function runRestoreLocked(options) {
  if (!existsSync(LINUX_APP)) fail(`installed app missing: ${LINUX_APP}`);
  assertNoLinuxApp();
  requireFreeSpace(options.linuxRoot);
  const candidateCommit = runCommandSync('git', ['rev-parse', '--verify', `${options.candidate}^{commit}`], { cwd: REPO }).stdout;
  const harnessCommit = runCommandSync('git', ['rev-parse', 'HEAD'], { cwd: REPO }).stdout;
  const drift = runCommandSync('git', ['diff', '--name-only', candidateCommit, harnessCommit, '--', ...APP_SOURCES], { cwd: REPO }).stdout;
  if (drift) fail(`app sources differ from candidate ${candidateCommit}:\n${drift}`);
  const fedoraPackage = fedoraPackageEvidence(candidateCommit);
  deployWindowsTools(options.sshHost);
  const recovered = remoteWorker(options.sshHost, {
    action: 'recover', root: WINDOWS_ROOT, runId: 'recovery', appPath: WINDOWS_APP,
  });

  const runId = new Date().toISOString().replaceAll(/[-:]/g, '').replace(/\.\d{3}Z$/, 'Z');
  const evidencePath = join(REPO, 'docs', 'reports', 'evidence', `alpha-harness-restore-${runId}.jsonl`);
  const screenshotDir = join(options.linuxRoot, 'evidence', `restore-${runId}`);
  mkdirSync(dirname(evidencePath), { recursive: true });
  mkdirSync(screenshotDir, { recursive: true });
  observation(evidencePath, 'run-start', 'controller', { runId, commit: candidateCommit, harnessCommit, killPoints: Object.keys(KILL_POINTS) });
  observation(evidencePath, 'package-evidence', 'fedora', fedoraPackage);
  observation(evidencePath, 'keep-awake', 'controller', {
    fedora: 'systemd-inhibit sleep:idle',
    windows: `PowerSetRequest SystemRequired, ${options.holdMinutes} min`,
  });
  if (recovered.recovered) observation(evidencePath, 'stale-run-recovered', 'windows', recovered);

  let runError;
  let cleanupError;
  const machines = [
    () => linuxRestoreMachine(options.linuxRoot, runId, randomUUID()),
    // Built before prepare runs, so a prepare that fails halfway is still finalized.
    () => windowsRestoreMachine(options.sshHost, runId, randomUUID(), candidateCommit),
  ];
  for (const makeMachine of machines) {
    let machine;
    try {
      machine = makeMachine();
      observation(evidencePath, 'prepared', machine.name, machine.prepare());
      await restoreGate(machine, evidencePath, screenshotDir);
    } catch (error) {
      runError = error;
      observation(evidencePath, 'run-failed', machine?.name ?? 'controller', { error: error.message });
    }
    try {
      const final = await machine?.cleanup(evidencePath);
      if (final) observation(evidencePath, 'process-final', machine.name, final);
    } catch (error) {
      observation(evidencePath, 'cleanup-error', machine?.name ?? 'controller', { error: error.message });
      cleanupError ??= error;
    }
    if (runError || cleanupError) break;
  }

  if (runError) throw runError;
  if (cleanupError) throw cleanupError;
  observation(evidencePath, 'run-complete', 'controller');
  console.log(JSON.stringify({ runId, evidencePath, screenshotDir }));
}

// Windows keeps the run's processes, junction, tasks, and swapped config until this succeeds;
// both gates end every run here, failed or not.
async function finalizeWindows(sshHost, runId, evidencePath) {
  let firstError;
  let processesStopped = false;
  try {
    let finalReport;
    try {
      finalReport = await waitForWindowsProcesses(sshHost, runId, (processes) => processes.length === 0, 60_000);
    } catch (error) {
      const forced = remoteWorker(sshHost, { action: 'cleanup-processes', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP });
      observation(evidencePath, 'forced-process-cleanup', 'windows', forced);
      finalReport = await waitForWindowsProcesses(sshHost, runId, (processes) => processes.length === 0, 15_000);
      firstError ??= error;
    }
    observation(evidencePath, 'process-final', 'windows', finalReport);
    processesStopped = true;
  } catch (error) {
    observation(evidencePath, 'cleanup-error', 'windows-processes', { error: error.message });
    firstError ??= error;
  }
  if (processesStopped) {
    try {
      const finalized = remoteWorker(sshHost, { action: 'finalize', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP });
      observation(evidencePath, 'config-restored', 'windows', finalized);
    } catch (error) {
      observation(evidencePath, 'cleanup-error', 'windows-config', { error: error.message });
      firstError ??= error;
    }
  }
  return firstError;
}

function windowsPaths(root, runId) {
  const runRoot = win32.join(root, 'runs', runId, 'windows');
  return {
    runRoot,
    vaultPath: win32.join(runRoot, 'vault'),
    backupPath: win32.join(runRoot, 'backups'),
    machinePath: win32.join(runRoot, 'machine-state'),
    manifestPath: win32.join(runRoot, 'state.json'),
    snapshotPath: win32.join(runRoot, 'vault-pre'),
  };
}

function assertWindowsRoot(root, path) {
  const base = `${win32.resolve(root).toLowerCase()}\\`;
  if (!win32.resolve(path).toLowerCase().startsWith(base)) fail(`path leaves test root: ${path}`);
}

function windowsPowerShell(tools, args) {
  const result = runCommandSync('powershell.exe', [
    '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
    '-File', win32.join(tools, 'alpha-harness.ps1'), ...args,
  ]);
  return parseLastJson(result.stdout);
}

function windowsInterrupt(tools, appPath, action, target) {
  const args = [
    '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
    '-File', win32.join(tools, 'sync-interrupt.ps1'),
    '-Action', action,
    '-InstallDir', dirname(appPath),
  ];
  if (target) args.push('-Target', target);
  return parseLastJson(runCommandSync('powershell.exe', args).stdout);
}

function windowsFindProcess(tools, executable) {
  return windowsPowerShell(tools, ['-Action', 'FindProcess', '-ExecutablePath', executable]).processes;
}

function windowsControl(paths) {
  return JSON.parse(readFileSync(win32.join(paths.machinePath, 'sync-control.json'), 'utf8'));
}

function windowsTailscaleIp() {
  const tailscale = existsSync('C:\\Program Files\\Tailscale\\tailscale.exe')
    ? 'C:\\Program Files\\Tailscale\\tailscale.exe'
    : 'tailscale.exe';
  return parseTailscaleIp(runCommandSync(tailscale, ['ip', '-4']).stdout);
}

// The run directory keeps all machine state as evidence; only the pointers into the user's
// profile and task list are removed. The junction is checked to target this run before rmdir,
// which removes the link and never its target.
function removeRunArtifacts(tools, appPath, manifest, paths) {
  let junctionRemoved = false;
  if (existsSync(manifest.machineLink)) {
    if (!lstatSync(manifest.machineLink).isSymbolicLink()
      || win32.resolve(readlinkSync(manifest.machineLink)).toLowerCase() !== win32.resolve(paths.machinePath).toLowerCase()) {
      fail(`refusing to remove ${manifest.machineLink}: not this run's junction`);
    }
    rmdirSync(manifest.machineLink);
    junctionRemoved = true;
  }
  windowsPowerShell(tools, ['-Action', 'RemoveTask', '-TaskName', WINDOWS_TASK, '-ExecutablePath', appPath]);
  if (existsSync(WINDOWS_DRIVER)) {
    windowsPowerShell(tools, ['-Action', 'RemoveTask', '-TaskName', WINDOWS_DRIVER_TASK, '-ExecutablePath', WINDOWS_DRIVER]);
  }
  return { machineState: paths.machinePath, junctionRemoved, taskRemoved: WINDOWS_TASK };
}

async function windowsWorker(request) {
  if (process.platform !== 'win32') fail('Windows worker must run on Windows');
  const root = win32.resolve(request.root);
  if (!root.toLowerCase().startsWith('d:\\secondbraintest\\')) fail('Windows root must be under D:\\SecondBrainTest');
  const tools = win32.join(root, 'tools');
  const paths = windowsPaths(root, request.runId);
  assertWindowsRoot(root, paths.runRoot);
  const appPath = request.appPath ? win32.resolve(request.appPath) : WINDOWS_APP;

  if (request.action === 'recover') {
    const configPath = win32.join(process.env.APPDATA, 'helixnotes', 'config.json');
    const lockPath = win32.join(dirname(configPath), 'alpha-harness.lock.json');
    if (!existsSync(lockPath)) return { recovered: false };
    const report = windowsInterrupt(tools, appPath, 'Report');
    if (report.processes.length) fail('stale harness lock exists while installed processes are running');
    const lock = JSON.parse(readFileSync(lockPath, 'utf8'));
    const manifest = JSON.parse(readFileSync(lock.manifestPath, 'utf8'));
    if (manifest.runId !== lock.runId || win32.resolve(manifest.configPath) !== win32.resolve(configPath)) {
      fail('stale harness journal is inconsistent');
    }
    atomicWrite(configPath, readFileSync(manifest.originalPath));
    unlinkSync(lockPath);
    return { recovered: true, runId: manifest.runId, runRoot: dirname(lock.manifestPath) };
  }

  if (request.action === 'prepare') {
    if (!existsSync(appPath)) fail(`installed app missing: ${appPath}`);
    requireFreeSpace(root);
    const system = windowsPowerShell(tools, ['-Action', 'Metadata', '-ExecutablePath', appPath]);
    if (!system.osCaption.includes('Windows 11') || !system.osArchitecture.includes('64')) {
      fail(`unsupported Windows target: ${system.osCaption} ${system.osArchitecture}`);
    }
    const app = binaryEvidence(appPath, request.candidateCommit);
    const existing = windowsInterrupt(tools, appPath, 'Report');
    if (existing.processes.length) fail('installed Windows app is already running');
    if (existsSync(paths.runRoot)) fail(`run already exists: ${paths.runRoot}`);
    mkdirSync(paths.backupPath, { recursive: true });
    mkdirSync(paths.machinePath, { recursive: true });
    makeVault(paths.vaultPath, request.vaultId);

    const configPath = win32.join(process.env.APPDATA, 'helixnotes', 'config.json');
    if (!existsSync(configPath)) fail(`installed app config is missing: ${configPath}`);
    const originalPath = win32.join(paths.runRoot, 'original-config.json');
    copyFileSync(configPath, originalPath);
    const original = JSON.parse(readFileSync(configPath, 'utf8'));
    const localVaults = win32.join(process.env.LOCALAPPDATA, 'helixnotes', 'vaults');
    const machineLink = win32.join(localVaults, request.vaultId);
    mkdirSync(localVaults, { recursive: true });
    if (existsSync(machineLink)) fail(`machine-state path already exists: ${machineLink}`);
    mkdirSync(dirname(machineLink), { recursive: true });
    const junction = runCommandSync('cmd.exe', ['/d', '/s', '/c', 'mklink', '/J', machineLink, paths.machinePath]);
    if (!junction.stdout) fail('could not create machine-state junction');

    const gate = request.gate ?? 'sync';
    if (gate === 'sync') {
      const control = makeControl(request.vaultId);
      atomicWrite(win32.join(paths.machinePath, 'sync-control.json'), `${JSON.stringify(control, null, 2)}\n`);
    }
    const lockPath = win32.join(dirname(configPath), 'alpha-harness.lock.json');
    const manifest = {
      version: 1, runId: request.runId, startedAt: new Date().toISOString(), configPath, originalPath, machineLink, appPath,
    };
    atomicWrite(paths.manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    writeFileSync(lockPath, `${JSON.stringify({ runId: request.runId, manifestPath: paths.manifestPath })}\n`, { flag: 'wx' });
    atomicWrite(
      configPath,
      `${JSON.stringify(harnessConfig(original, paths.vaultPath, request.vaultId, paths.backupPath, gate), null, 2)}\n`,
    );
    return {
      runRoot: paths.runRoot,
      vault: paths.vaultPath,
      freeBytes: requireFreeSpace(root),
      package: { ...system, app },
    };
  }

  if (request.action === 'check') {
    try {
      return { observation: check(paths.vaultPath) };
    } catch (error) {
      return {
        observation: {
          at: new Date().toISOString(), vault: paths.vaultPath, expected: NOTE_COUNT,
          complete: false, error: error.message,
        },
      };
    }
  }
  if (request.action === 'report') return windowsInterrupt(tools, appPath, 'Report');
  if (request.action === 'interrupt-sidecar') return windowsInterrupt(tools, appPath, 'Stop', 'Sidecar');

  if (request.action === 'launch') {
    windowsPowerShell(tools, ['-Action', 'EnsureTask', '-TaskName', WINDOWS_TASK, '-ExecutablePath', appPath]);
    windowsPowerShell(tools, ['-Action', 'RunTask', '-TaskName', WINDOWS_TASK, '-ExecutablePath', appPath]);
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline) {
      const report = windowsInterrupt(tools, appPath, 'Report');
      const apps = report.processes.filter((process) => process.Role === 'App');
      if (apps.length === 1) return { pid: apps[0].Id, path: apps[0].Path };
      await sleep(500);
    }
    fail('scheduled task did not launch exactly one app process');
  }

  if (request.action === 'stop-app') {
    return windowsPowerShell(tools, [
      '-Action', 'StopPid', '-PidToStop', String(request.pid), '-ExecutablePath', appPath,
    ]);
  }

  if (request.action === 'cleanup-processes') {
    // AGENTS.md: stop only what this run started. The sidecar and watchdog are the app's
    // children, so their PIDs are never handed to us; exact package path plus a start time
    // inside this run is the closest proof available.
    const { startedAt } = JSON.parse(readFileSync(paths.manifestPath, 'utf8'));
    const reported = windowsInterrupt(tools, appPath, 'Report');
    const before = { processes: reported.processes.filter((record) => new Date(record.Started) >= new Date(startedAt)) };
    const untouched = reported.processes.filter((record) => !before.processes.includes(record));
    if (untouched.length) fail(`refusing: installed processes predate this run: ${JSON.stringify(untouched)}`);
    for (const role of ['App', 'Watchdog', 'Sidecar', 'SidecarOrphan']) {
      for (const processRecord of before.processes.filter((process) => process.Role === role)) {
        try {
          windowsPowerShell(tools, [
            '-Action', 'StopPid', '-PidToStop', String(processRecord.Id),
            '-ExecutablePath', processRecord.Path,
          ]);
        } catch (error) {
          if (!String(error.message).includes('is not running')) throw error;
        }
      }
    }
    return { before: before.processes, after: windowsInterrupt(tools, appPath, 'Report').processes };
  }

  if (request.action === 'identity') {
    const control = windowsControl(paths);
    return waitForSidecar(control, windowsTailscaleIp, 'Windows');
  }

  if (request.action === 'configure') {
    const controlPath = win32.join(paths.machinePath, 'sync-control.json');
    const control = windowsControl(paths);
    control.peer = { deviceId: request.peerId, name: request.peerName, tailscaleIp: request.peerIp };
    atomicWrite(controlPath, `${JSON.stringify(control, null, 2)}\n`);
    const current = await syncthingRequest(control, 'GET', '/rest/config');
    const next = pairedSyncthingConfig(current, {
      vaultId: readFileSync(win32.join(paths.vaultPath, '.helixnotes', 'vault_id'), 'utf8').trim(),
      vaultPath: paths.vaultPath,
      peerId: request.peerId,
      peerName: request.peerName,
      peerIp: request.peerIp,
      localIp: windowsTailscaleIp(),
    });
    await syncthingRequest(control, 'PUT', '/rest/config', next);
    return syncthingRequest(control, 'GET', '/rest/config/restart-required');
  }

  if (request.action === 'backups') {
    return { backups: readdirSync(paths.backupPath).filter((name) => name.startsWith('helixnotes-pre-sync-')) };
  }

  if (request.action === 'generate') return generate(paths.vaultPath);
  if (request.action === 'hash') return treeHash(paths.vaultPath);
  if (request.action === 'progress') return restoreProgress(paths.vaultPath);
  if (request.action === 'mutate') return mutateFixture(paths.vaultPath);
  if (request.action === 'all-backups') return { backups: readdirSync(paths.backupPath) };
  if (request.action === 'snapshot') {
    if (existsSync(paths.snapshotPath)) fail(`snapshot already exists: ${paths.snapshotPath}`);
    cpSync(paths.vaultPath, paths.snapshotPath, { recursive: true });
    return { snapshot: paths.snapshotPath };
  }
  if (request.action === 'reset') {
    resetVault(paths.vaultPath, paths.snapshotPath, paths.runRoot);
    return { reset: paths.vaultPath };
  }

  if (request.action === 'start-driver') {
    const { startedAt } = JSON.parse(readFileSync(paths.manifestPath, 'utf8'));
    const running = windowsFindProcess(tools, WINDOWS_DRIVER);
    if (running.length) fail(`tauri-driver is already running: ${JSON.stringify(running)}`);
    windowsPowerShell(tools, [
      '-Action', 'EnsureTask', '-TaskName', WINDOWS_DRIVER_TASK, '-ExecutablePath', WINDOWS_DRIVER,
      '-TaskArguments', `--port ${WINDOWS_DRIVER_PORT} --native-driver "${WINDOWS_NATIVE_DRIVER}"`,
    ]);
    windowsPowerShell(tools, ['-Action', 'RunTask', '-TaskName', WINDOWS_DRIVER_TASK, '-ExecutablePath', WINDOWS_DRIVER]);
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline) {
      const drivers = windowsFindProcess(tools, WINDOWS_DRIVER)
        .filter((record) => new Date(record.Started) >= new Date(startedAt));
      if (drivers.length === 1) {
        // WebView2 and its WebDriver need the interactive desktop; session 0 has none.
        if (drivers[0].SessionId === 0) fail('tauri-driver started in session 0, not the desktop');
        return { pid: drivers[0].Id, sessionId: drivers[0].SessionId };
      }
      await sleep(500);
    }
    fail('scheduled task did not start exactly one tauri-driver');
  }

  if (request.action === 'app-pid') {
    const natives = windowsFindProcess(tools, WINDOWS_NATIVE_DRIVER).filter((record) => record.ParentId === request.driverPid);
    if (natives.length !== 1) fail(`expected one msedgedriver under tauri-driver ${request.driverPid}: ${JSON.stringify(natives)}`);
    const apps = windowsInterrupt(tools, appPath, 'Report').processes
      .filter((record) => record.Role === 'App' && record.ParentId === natives[0].Id);
    if (apps.length !== 1) fail(`expected one app under msedgedriver ${natives[0].Id}: ${JSON.stringify(apps)}`);
    return { pid: apps[0].Id };
  }

  if (request.action === 'stop-driver') {
    // Exact PIDs only: the driver this run started, then the msedgedriver it spawned.
    const stopped = [];
    for (const native of windowsFindProcess(tools, WINDOWS_NATIVE_DRIVER).filter((record) => record.ParentId === request.driverPid)) {
      windowsPowerShell(tools, ['-Action', 'StopPid', '-PidToStop', String(native.Id), '-ExecutablePath', WINDOWS_NATIVE_DRIVER]);
      stopped.push(native.Id);
    }
    if (windowsFindProcess(tools, WINDOWS_DRIVER).some((record) => record.Id === request.driverPid)) {
      windowsPowerShell(tools, ['-Action', 'StopPid', '-PidToStop', String(request.driverPid), '-ExecutablePath', WINDOWS_DRIVER]);
      stopped.push(request.driverPid);
    }
    windowsPowerShell(tools, ['-Action', 'RemoveTask', '-TaskName', WINDOWS_DRIVER_TASK, '-ExecutablePath', WINDOWS_DRIVER]);
    return { stopped };
  }

  if (request.action === 'finalize') {
    const report = windowsInterrupt(tools, appPath, 'Report');
    if (report.processes.length) fail('refusing to restore config while installed processes remain');
    const manifest = JSON.parse(readFileSync(paths.manifestPath, 'utf8'));
    if (manifest.runId !== request.runId || win32.resolve(manifest.configPath) !== win32.resolve(win32.join(process.env.APPDATA, 'helixnotes', 'config.json'))) {
      fail('run journal does not match requested config');
    }
    const lockPath = win32.join(dirname(manifest.configPath), 'alpha-harness.lock.json');
    if (!existsSync(lockPath)) {
      const current = readFileSync(manifest.configPath);
      const original = readFileSync(manifest.originalPath);
      if (!current.equals(original)) fail('config lock is gone but original config is not restored');
      return { restored: true, alreadyRestored: true, ...removeRunArtifacts(tools, appPath, manifest, paths) };
    }
    const lock = JSON.parse(readFileSync(lockPath, 'utf8'));
    if (lock.runId !== request.runId) fail('config lock belongs to another run');
    atomicWrite(manifest.configPath, readFileSync(manifest.originalPath));
    unlinkSync(lockPath);
    return { restored: true, ...removeRunArtifacts(tools, appPath, manifest, paths) };
  }

  fail(`unknown Windows action: ${request.action}`);
}

async function main() {
  const [commandName, ...args] = process.argv.slice(2);
  if (commandName === 'sync') return runSync(args);
  if (commandName === 'restore') return runRestore(args);
  if (commandName === '__kill-watch') {
    console.log(JSON.stringify(await killWatch(JSON.parse(Buffer.from(args[0], 'base64url').toString('utf8')))));
    return;
  }
  if (commandName === '__windows-worker') {
    const request = JSON.parse(Buffer.from(args[0], 'base64url').toString('utf8'));
    const result = await windowsWorker(request);
    console.log(JSON.stringify(result));
    return;
  }
  console.error('usage: node scripts/alpha-harness.mjs sync|restore [--candidate <sha>] [--ssh sb-windows] [--linux-root ~/sb88] [--timeout-minutes 20]');
  process.exitCode = 2;
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exitCode = 1;
  });
}
