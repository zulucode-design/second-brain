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
import { crc32, deflateSync, inflateRawSync } from 'node:zlib';
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
const WINDOWS_DESKTOP_TASK = 'SecondBrainAlphaHarnessDesktop';
const DEFAULT_WINDOWS_OLLAMA_PORT = 11434;
// Fedora reaches the desktop's Ollama through the controller's own tunnel on this port.
const FEDORA_OLLAMA_PORT = 11435;
// Nothing listens on the discard port, so the app sees its embedding backend as offline.
const OFFLINE_OLLAMA_URL = 'http://127.0.0.1:9';
const WINDOWS_TUNNEL_PORT = 4446;
const LINUX_DRIVER_PORT = 4444;
// Explicit, so a stray listener on tauri-driver's default (4445) fails loudly instead of being proxied to.
const LINUX_NATIVE_PORT = 4447;
// The notification AUMID of an installed package, and the name of its desktop entry.
const APP_IDENTIFIER = 'io.github.zulucodedesign.SecondBrain';
const LINUX_DESKTOP_ENTRY = `/usr/share/applications/${APP_IDENTIFIER}.desktop`;
const APP_SOURCES = ['src', 'src-tauri', 'static', 'pnpm-lock.yaml', 'svelte.config.js', 'vite.config.ts'];
const POLL_MS = 1_000;
const DEVICE_ID = /^[A-Z2-7]{7}(?:-[A-Z2-7]{7}){7}$/;

function fail(message) {
  throw new Error(message);
}

function windowsOllamaUrl(port) {
  if (!Number.isInteger(port) || port < 1024 || port > 65535) fail('--ollama-port must be an integer from 1024 to 65535');
  return `http://127.0.0.1:${port}`;
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

// `driven`: a single-machine run under tauri-driver (Gates 2 and 3). It restores only backups it
// made, so a scheduled backup would add one it did not expect.
export function harnessConfig(config, vaultPath, vaultId, backupPath, { driven = false, ollamaBaseUrl } = {}) {
  const next = structuredClone(config);
  delete next.vaults;
  next.vault = { path: vaultPath, name: 'Alpha Harness', vault_id: vaultId };
  next.active_vault = vaultPath;
  delete next.active_bookmark_id;
  next.backup_location = backupPath;
  next.backup_max_count = Math.max(10, Number(next.backup_max_count) || 0);
  if (driven) {
    next.backup_enabled = false;
    // The window's close button must end the app, not hide it in the tray.
    next.close_to_tray = false;
  }
  if (ollamaBaseUrl) next.ollama_base_url = ollamaBaseUrl;
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
    type: 'sendonly',
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
  const path = join(configHome, APP_IDENTIFIER, 'config.json');
  if (!existsSync(path)) fail(`installed app config is missing: ${path}`);
  return JSON.parse(readFileSync(path, 'utf8'));
}

// Package F: a config.json that is not JSON must surface as a startup error, not a fresh start.
const MALFORMED_CONFIG = '{ "vaults": [ not json\n';
// The run's config, kept while MALFORMED_CONFIG is in its place.
const GOOD_CONFIG_NAME = 'config-before-break.json';

function patchConfigFile(configPath, patch) {
  const current = JSON.parse(readFileSync(configPath, 'utf8'));
  atomicWrite(configPath, `${JSON.stringify({ ...current, ...patch }, null, 2)}\n`);
  return Object.keys(patch);
}
// The app keeps a config it could not parse beside the new one, under this prefix.
const DAMAGED_CONFIG_PREFIX = 'config.json.damaged-';

function prepareLinux(root, runId, vaultId, { driven = false, sync = !driven, ollamaBaseUrl } = {}) {
  requireFreeSpace(root);
  const runRoot = join(root, 'runs', runId, 'fedora');
  if (existsSync(runRoot)) fail(`run already exists: ${runRoot}`);
  const vaultPath = join(runRoot, 'vault');
  const backupPath = join(runRoot, 'backups');
  const configHome = join(runRoot, 'xdg-config');
  const dataHome = join(runRoot, 'xdg-data');
  const configPath = join(configHome, APP_IDENTIFIER, 'config.json');
  const machinePath = join(dataHome, APP_IDENTIFIER, 'vaults', vaultId);
  mkdirSync(dirname(configPath), { recursive: true });
  mkdirSync(machinePath, { recursive: true });
  mkdirSync(backupPath, { recursive: true });
  makeVault(vaultPath, vaultId);
  // Without a control file sync stays off. The restore gate runs so, with no sidecar; the
  // walkthrough turns sync on, unpaired, so the sidecar and watchdog run and must exit.
  const control = sync ? makeControl(vaultId) : null;
  if (control) atomicWrite(join(machinePath, 'sync-control.json'), `${JSON.stringify(control, null, 2)}\n`);
  atomicWrite(
    configPath,
    `${JSON.stringify(harnessConfig(localTemplateConfig(), vaultPath, vaultId, backupPath, { driven, ollamaBaseUrl }), null, 2)}\n`,
  );
  return { runRoot, vaultPath, backupPath, configHome, dataHome, machinePath, control, configPath };
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
// by systemd-inhibit, with GNOME's idle lock inhibited while screenshots run. Windows uses a power
// request whose holder the controller stops by its reported PID and which expires if it dies.
async function keepAwake(sshHost, timeoutMs) {
  const holdMinutes = Math.ceil((3 * timeoutMs) / 60_000) + 15;
  const fedora = spawn('systemd-inhibit', [
    '--what=sleep:idle', '--who=alpha-harness', `--why=${WINDOWS_KEEP_AWAKE_REASON}`, 'sleep', 'infinity',
  ], { stdio: 'ignore' });
  const gnome = spawn('gnome-session-inhibit', [
    '--inhibit=idle', `--reason=${WINDOWS_KEEP_AWAKE_REASON}`, '--inhibit-only',
  ], { stdio: 'ignore' });
  const windows = spawn('ssh', [
    sshHost, 'powershell.exe', '-NoProfile', '-NonInteractive', '-EncodedCommand',
    encodedPowerShell(windowsKeepAwake(holdMinutes)),
  ], { stdio: ['ignore', 'pipe', 'ignore'] });
  let holderPid;
  let lost;
  const release = () => {
    fedora.kill();
    gnome.kill();
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
      gnome.on('exit', () => { clearTimeout(timer); rejectHeld(new Error('GNOME idle inhibitor exited before the run')); });
    });
  } catch (error) {
    release();
    throw error;
  }
  // Losing a hold mid-run means the machine may sleep again; the run reports it instead of
  // passing quietly.
  fedora.on('exit', () => { lost ??= 'systemd-inhibit exited during the run'; });
  gnome.on('exit', () => { lost ??= 'GNOME idle inhibitor exited during the run'; });
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
    join(REPO, 'scripts', 'windows', 'alpha-desktop.ps1'),
    join(REPO, 'scripts', 'windows', 'app-isolation.ps1'),
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

// Public: committed under docs/reports/evidence, so observation() redacts every line.
function evidencePathFor(gate, runId) {
  return join(REPO, 'docs', 'reports', 'evidence', `alpha-harness-${gate}-${runId}.jsonl`);
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

// With sync on, the app starts its Syncthing sidecar and the watchdog that outlives a crash.
// Reads until `predicate` holds for what `read` returns.
async function waitForState(read, predicate, timeoutMs, what) {
  const deadline = Date.now() + timeoutMs;
  let state;
  while (Date.now() < deadline) {
    // Fedora's Syncthing reads are async; a pending promise never satisfies the predicate.
    state = await read();
    if (predicate(state)) return state;
    await sleep(500);
  }
  fail(`${what} timed out: ${JSON.stringify(state)}`);
}

// With sync on, the app starts its Syncthing sidecar and the watchdog that outlives a crash.
const syncProcessesUp = (roles) => roles.includes('Sidecar') && roles.includes('Watchdog');

function windowsReport(sshHost, runId) {
  return remoteWorker(sshHost, { action: 'report', root: WINDOWS_ROOT, runId });
}

function waitForWindowsProcesses(sshHost, runId, predicate, timeoutMs = 30_000) {
  return waitForState(() => windowsReport(sshHost, runId), (report) => predicate(report.processes), timeoutMs, 'Windows process state');
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

function killForCleanup(pid, signal) {
  try {
    process.kill(pid, signal);
  } catch (error) {
    if (error.code !== 'ESRCH') throw error;
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
      const killedAt = process.platform === 'win32'
        ? windowsPowerShell(WINDOWS_TOOLS, [
            '-Action', 'StopPid', '-PidToStop', String(pid), '-ExecutablePath', WINDOWS_APP,
          ]).at
        : (process.kill(pid, 'SIGKILL'), new Date().toISOString());
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

export function snapshotVault(vault, snapshot) {
  if (existsSync(snapshot)) fail(`snapshot already exists: ${snapshot}`);
  cpSync(vault, snapshot, { recursive: true });
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

async function openApp(port, application, readySelector = 'button=New Note') {
  // Only the controller needs webdriverio; the Windows worker copy runs without node_modules.
  const { remote } = await import('webdriverio');
  const browser = await remote({
    hostname: '127.0.0.1',
    port,
    logLevel: 'warn',
    connectionRetryCount: 0,
    capabilities: { 'tauri:options': { application } },
  });
  // Resize before waiting: WebDriver's default window, or a persisted compact one, hides the
  // wide-layout ready selector and folds the editor toolbar away.
  await browser.setWindowSize(1280, 860);
  await browser.$(readySelector).waitForDisplayed({ timeout: 60_000 });
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
async function readAndDismissNotice(browser, screenshotPath) {
  const banner = browser.$('.repair-banner');
  let displayed = true;
  try {
    await banner.waitForDisplayed({ timeout: 20_000 });
  } catch {
    displayed = false;
  }
  await browser.saveScreenshot(screenshotPath);
  if (!displayed) return null;
  const notice = { title: await banner.$('strong').getText(), message: await banner.$('span').getText() };
  const started = Date.now();
  await press(browser, banner.$('button=Dismiss'));
  await banner.waitForExist({ reverse: true, timeout: 60_000 });
  return { ...notice, dismissedMs: Date.now() - started };
}

export function parseOptions(args) {
  const options = {
    sshHost: 'sb-windows',
    linuxRoot: join(homedir(), 'sb88'),
    timeoutMs: DEFAULT_TIMEOUT_MS,
    ollamaPort: DEFAULT_WINDOWS_OLLAMA_PORT,
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
    else if (name === '--ollama-port') options.ollamaPort = Number(value);
    else if (name === '--windows-installer') options.windowsInstaller = value;
    else if (name === '--fedora-rpm') options.fedoraRpm = resolve(value);
    else if (name === '--run') options.runId = value;
    else if (name === '--machine') options.machine = value;
    else fail(`unknown option: ${name}`);
  }
  if (!Number.isFinite(options.timeoutMs) || options.timeoutMs <= 0) fail('timeout must be a positive number');
  windowsOllamaUrl(options.ollamaPort);
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
  return runOnFedora(args, runSyncLocked);
}

async function runOnFedora(args, runLocked) {
  const options = parseOptions(args);
  if (process.platform !== 'linux') fail('harness controller must run on Fedora');
  mkdirSync(options.linuxRoot, { recursive: true });
  const lockRoot = join(homedir(), '.cache', 'second-brain');
  mkdirSync(lockRoot, { recursive: true });
  const releaseLock = acquireControllerLock(lockRoot);
  let awake;
  try {
    awake = await keepAwake(options.sshHost, options.timeoutMs);
    const result = await runLocked({ ...options, holdMinutes: awake.holdMinutes });
    if (awake.lost()) fail(`${awake.lost()}; the result cannot be trusted`);
    return result;
  } finally {
    awake?.release();
    releaseLock();
  }
}

function prepareHarnessRun(options) {
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
  return { candidateCommit, harnessCommit, fedoraPackage, recovered, runId };
}

async function runSyncLocked(options) {
  const { candidateCommit, harnessCommit, fedoraPackage, recovered, runId } = prepareHarnessRun(options);
  const vaultId = randomUUID();
  const evidencePath = evidencePathFor('sync', runId);
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
    fedora: 'systemd-inhibit sleep:idle; gnome-session-inhibit idle',
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

    const beforeStop = windowsReport(options.sshHost, runId);
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
    // A machine backs up only before a batch that brings it changes (#14). Windows receives the
    // fixture, so it must have one; Fedora has one only if something reached it too.
    if (!windowsBackups.length) fail('pre-sync backup missing on the receiving machine');
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

function linuxDriverMachine(root, runId, vaultId, { ollamaBaseUrl, sync = false } = {}) {
  const machine = prepareLinux(root, runId, vaultId, { driven: true, sync, ollamaBaseUrl });
  const goodConfigPath = join(machine.runRoot, GOOD_CONFIG_NAME);
  const snapshotPath = join(machine.runRoot, 'vault-pre');
  const notionAttributes = ['service', APP_IDENTIFIER, 'username', `integration:notion:${vaultId}`];
  let driver;
  let entryLaunchAttempted = false;
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
    snapshot: () => snapshotVault(machine.vaultPath, snapshotPath),
    reset: () => resetVault(machine.vaultPath, snapshotPath, machine.runRoot),
    readNote: (relativePath) => readFileSync(join(machine.vaultPath, relativePath), 'utf8'),
    session: fedoraSession,
    async launchEntry() {
      assertNoLinuxApp();
      const logPath = join(machine.runRoot, 'desktop-entry.log');
      const log = openSync(logPath, 'a');
      entryLaunchAttempted = true;
      const launcher = spawn('gio', ['launch', LINUX_DESKTOP_ENTRY], {
        env: { ...process.env, XDG_CONFIG_HOME: machine.configHome, XDG_DATA_HOME: machine.dataHome },
        stdio: ['ignore', log, log],
      });
      closeSync(log);
      let launchError;
      launcher.on('error', (error) => { launchError = error; });
      try {
        const [app] = await waitForState(
          () => {
            if (launchError) throw launchError;
            const rows = processRows().filter((row) => row.executable === LINUX_APP
              && !row.commandLine.includes('--helix-sync-watchdog'));
            if (!rows.length && launcher.exitCode !== null) fail(`gio launch exited ${launcher.exitCode}: ${readFileSync(logPath, 'utf8')}`);
            return rows;
          },
          (rows) => rows.length === 1, 30_000, 'Fedora desktop entry launch',
        );
        await sleep(2_000);
        if (!processRows().some((row) => row.pid === app.pid && row.executable === LINUX_APP)) {
          fail('Fedora desktop entry app exited after launch');
        }
        return { entry: LINUX_DESKTOP_ENTRY, executable: app.executable, pid: app.pid };
      } finally {
        // No app existed before this entry launch. Stop only exact PIDs it started.
        for (const row of linuxReport(machine.machinePath)) killForCleanup(row.pid, 'SIGTERM');
        if (launcher.exitCode === null) launcher.kill('SIGTERM');
        await this.appsGone();
        entryLaunchAttempted = false;
      }
    },
    syncRunning: async () => ({
      roles: await waitForState(() => linuxReport(machine.machinePath).map((row) => row.role), syncProcessesUp, 60_000, 'sync processes'),
    }),
    // Fedora's webview takes the file object itself, so nothing has to be staged on disk.
    stageFile: () => null,
    stageBinary: () => null,
    // #28 acceptance reads: what Syncthing, the vault, and the backups hold. Nothing here acts.
    syncthing: (method, path, body) => syncthingRequest(
      JSON.parse(readFileSync(join(machine.machinePath, 'sync-control.json'), 'utf8')), method, path, body,
    ),
    syncRoles: () => linuxReport(machine.machinePath).map(({ role, pid, executable }) => ({ role, pid, executable })),
    vaultSummary: (markers) => vaultSummary(machine.vaultPath, markers),
    backupSummary: (name, wanted, markers) => backupSummary(join(machine.backupPath, name), wanted, markers),
    // The app's keyring entry: service is the app identifier, username the vault's account.
    notionTokenStored: () => runCommandSync('secret-tool', ['lookup', ...notionAttributes], { accept: [0, 1] }).status === 0,
    clearNotionToken() {
      const found = this.notionTokenStored();
      if (found) runCommandSync('secret-tool', ['clear', ...notionAttributes]);
      return { found, left: this.notionTokenStored() };
    },
    patchConfig: (patch) => patchConfigFile(machine.configPath, patch),
    breakConfig() {
      copyFileSync(machine.configPath, goodConfigPath);
      atomicWrite(machine.configPath, MALFORMED_CONFIG);
    },
    repairConfig() {
      // The app keeps the damaged file beside the config and writes defaults; both are replaced.
      const damaged = damagedConfigs(dirname(machine.configPath));
      for (const name of damaged) renameSync(join(dirname(machine.configPath), name), join(machine.runRoot, name));
      atomicWrite(machine.configPath, readFileSync(goodConfigPath));
      return { damaged };
    },
    diagnosticsPath: join(machine.runRoot, 'diagnostics.zip'),
    fetchDiagnostics(destination) {
      copyFileSync(this.diagnosticsPath, destination);
      return readFileSync(destination);
    },
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
      if (entryLaunchAttempted) {
        for (const row of linuxReport(machine.machinePath)) killForCleanup(row.pid, 'SIGKILL');
      }
      if (!driver) return this.appsGone();
      // Exact PIDs only: apps under this run's driver, then the driver's own children.
      for (const row of apps()) killForCleanup(row.pid, 'SIGKILL');
      for (const row of processRows().filter((candidate) => candidate.parentPid === driver.pid)) {
        killForCleanup(row.pid, 'SIGTERM');
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

function windowsDriverMachine(sshHost, runId, vaultId, candidateCommit, { ollamaBaseUrl, sync = false } = {}) {
  const runWindowsAction = (action, extra = {}, timeout = 45_000) => remoteWorker(
    sshHost, { action, root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP, ...extra }, timeout,
  );
  const longTimeoutMs = 5 * 60_000;
  // Desktop-session actions wait for a scheduled task in the interactive session.
  const desktopTimeoutMs = 6 * 60_000;
  const fetchRemote = (remote, destination) => {
    runCommandSync('scp', [`${sshHost}:${remote.replaceAll('\\', '/')}`, destination], { timeout: 60_000 });
    return readFileSync(destination);
  };
  const diagnosticsPath = win32.join(windowsPaths(WINDOWS_ROOT, runId).runRoot, 'diagnostics.zip');
  let driverPid;
  let tunnel;
  return {
    name: 'windows',
    application: WINDOWS_APP,
    port: WINDOWS_TUNNEL_PORT,
    prepare: () => runWindowsAction('prepare', { vaultId, candidateCommit, driven: true, sync, ollamaBaseUrl }),
    generate: () => runWindowsAction('generate', {}, longTimeoutMs),
    hash: () => runWindowsAction('hash', {}, longTimeoutMs),
    progress: () => runWindowsAction('progress'),
    mutate: () => runWindowsAction('mutate', {}, longTimeoutMs),
    backups: () => runWindowsAction('all-backups').backups,
    snapshot: () => runWindowsAction('snapshot', {}, longTimeoutMs),
    reset: () => runWindowsAction('reset', {}, longTimeoutMs),
    readNote: (relativePath) => runWindowsAction('read-note', { relativePath }).content,
    session: () => runWindowsAction('session'),
    launchEntry: () => runWindowsAction('launch-entry', {}, desktopTimeoutMs),
    syncRunning: async () => {
      const roles = (processes) => processes.map((row) => row.Role);
      const report = await waitForWindowsProcesses(sshHost, runId, (processes) => syncProcessesUp(roles(processes)), 60_000);
      return { roles: roles(report.processes) };
    },
    stageFile: (name, content) => runWindowsAction('stage-file', { name, content }).path,
    stageBinary: (name, base64) => runWindowsAction('stage-file', { name, base64 }).path,
    syncthing: (method, path, body) => runWindowsAction('syncthing', { method, path, body }).value,
    syncRoles: () => windowsReport(sshHost, runId).processes.map(({ Role, Id, Path }) => ({ role: Role, pid: Id, executable: Path })),
    vaultSummary: (markers) => runWindowsAction('vault-summary', { markers }, longTimeoutMs),
    backupSummary: (name, wanted, markers) => runWindowsAction('backup-summary', { name, wanted, markers }, longTimeoutMs),
    isolation: (mode, programs) => runWindowsAction('isolation', { mode, programs }, 2 * 60_000),
    connections: () => runWindowsAction('connections'),
    notionTokenStored: () => runWindowsAction('notion-token', { vaultId, remove: false }, desktopTimeoutMs).found,
    clearNotionToken: () => runWindowsAction('notion-token', { vaultId, remove: true }, desktopTimeoutMs),
    // #49: with the vault folder renamed away, the capture hotkey must raise the installed app's
    // "vault unavailable" toast. The key press comes from the desktop session, like a user's.
    async hotkeyCheck(screenshot) {
      const renamed = runWindowsAction('rename-vault', { away: true });
      let pressed;
      try {
        pressed = await withApp(this, async () => {
          // Startup has to reach hotkey registration before the chord is sent.
          await sleep(5_000);
          return runWindowsAction('desktop', { mode: 'CaptureHotkey', image: 'hotkey-toast.png' }, desktopTimeoutMs);
        }, 'body');
      } catch (error) {
        // Put the vault back without letting a failed rename hide why the hotkey failed.
        try { runWindowsAction('rename-vault', { away: false }); } catch {}
        throw error;
      }
      runWindowsAction('rename-vault', { away: false });
      fetchRemote(pressed.image, screenshot);
      // History keeps earlier runs' toasts, so the proof is a toast naming this run's vault. The
      // app showed one per press, as its log says, yet the history holds one after each press:
      // the second replaced the first (#153).
      const { before, afterFirst, after } = pressed.toasts;
      const captures = (toasts) => (toasts ?? []).filter((toast) => toast.startsWith('Quick capture')
        && toast.includes("vault isn't available") && toast.includes(renamed.from));
      const counts = { before: captures(before).length, afterFirst: captures(afterFirst).length, after: captures(after).length };
      if (pressed.toastsShown !== 2 || counts.before !== 0 || counts.afterFirst !== 1 || counts.after !== 1) {
        fail(`expected two toasts shown and one kept for ${renamed.from}: ${JSON.stringify({ toastsShown: pressed.toastsShown, counts, toasts: pressed.toasts })}`);
      }
      return { renamed: renamed.to, toast: captures(after)[0], toastsShown: pressed.toastsShown, counts };
    },
    install: (installer) => runWindowsAction('install', { installer, candidateCommit }, desktopTimeoutMs),
    uninstall: () => runWindowsAction('uninstall', {}, 3 * 60_000),
    patchConfig: (patch) => runWindowsAction('patch-config', { patch }).patched,
    breakConfig: () => runWindowsAction('break-config'),
    repairConfig: () => runWindowsAction('repair-config'),
    diagnosticsPath,
    fetchDiagnostics: (destination) => fetchRemote(diagnosticsPath, destination),
    async startDriver() {
      const started = runWindowsAction('start-driver');
      driverPid = started.pid;
      tunnel = spawn('ssh', [
        '-N', '-o', 'ExitOnForwardFailure=yes',
        '-L', `127.0.0.1:${WINDOWS_TUNNEL_PORT}:127.0.0.1:${WINDOWS_DRIVER_PORT}`, sshHost,
      ], { stdio: 'ignore' });
      await waitForDriver(WINDOWS_TUNNEL_PORT);
      if (tunnel.exitCode !== null) fail(`WebDriver tunnel exited ${tunnel.exitCode}; is port ${WINDOWS_TUNNEL_PORT} taken?`);
      return started;
    },
    appPid: () => runWindowsAction('app-pid', { driverPid }).pid,
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
          observation(evidencePath, 'driver-stopped', 'windows', runWindowsAction('stop-driver', { driverPid }));
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

async function withApp(machine, action, readySelector) {
  const browser = await openApp(machine.port, machine.application, readySelector);
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
  const record = (event, value) => observation(evidencePath, event, machine.name, value);
  record('fixture-generated', await machine.generate());
  record('driver-started', await machine.startDriver());

  await withApp(machine, backupThroughUi);
  const backups = machine.backups();
  if (backups.length !== 1) fail(`expected one backup, found ${JSON.stringify(backups)}`);
  const backedUpState = machine.hash();
  record('backup-created', { backups, vault: backedUpState });

  record('vault-mutated', machine.mutate());
  const preRestoreState = await settle(machine);
  machine.snapshot();
  record('pre-state', preRestoreState);

  await withApp(machine, async (browser) => {
    await press(browser, await openRestoreConfirmation(browser));
    await waitForRestore(browser);
  });
  const restoredState = await settle(machine);
  const controlProgress = machine.progress();
  record('post-state', { vault: restoredState, leftovers: controlProgress.leftovers });
  if (restoredState.sha256 !== backedUpState.sha256) fail('the restore did not reproduce the backed-up vault');
  if (controlProgress.leftovers.length) fail(`completed restore left files behind: ${controlProgress.leftovers}`);

  for (const point of Object.keys(KILL_POINTS)) {
    machine.reset();
    if (machine.hash().sha256 !== preRestoreState.sha256) fail('vault reset did not reproduce the pre-state');
    let killed;
    await withApp(machine, async (browser) => {
      const confirm = await openRestoreConfirmation(browser);
      const pid = machine.appPid();
      const watch = machine.killWatch({ point, expectedFiles: backedUpState.files, pid, timeoutMs: 5 * 60_000 });
      await watch.ready;
      await press(browser, confirm);
      killed = await watch.result;
    });
    record('app-killed', killed);
    // A journal that outlives the app is the proof the kill landed inside the restore.
    if (killed.missed || killed.afterKill.phase === null) fail(`kill at ${point} landed after the restore finished`);
    record('interrupted-state', { point, vault: machine.hash(), progress: machine.progress() });

    const banner = await withApp(machine, async (relaunched) => {
      return readAndDismissNotice(relaunched, join(screenshotDir, `${machine.name}-${point}.png`));
    });
    const recoveryProgress = machine.progress();
    // Recorded before settling, so the evidence shows what recovery alone produced.
    const immediateState = machine.hash();
    const recoveredState = await settle(machine, immediateState);
    const outcome = recoveryOutcome(recoveredState.sha256, preRestoreState.sha256, restoredState.sha256);
    record('recovered', {
      point, outcome, immediate: immediateState, vault: recoveredState, leftovers: recoveryProgress.leftovers, banner,
    });
    if (!outcome) fail(`vault after recovery from ${point} is neither the pre-restore nor the restored state`);
    if (recoveryProgress.leftovers.length) fail(`recovery from ${point} left files behind: ${recoveryProgress.leftovers}`);
    if (banner?.title !== 'Restore interrupted' || !banner.message.includes(RECOVERY_NOTICE[outcome])) {
      fail(`recovery notice after ${point} is missing or wrong: ${JSON.stringify(banner)}`);
    }
  }
}

async function runRestore(args) {
  return runOnFedora(args, runRestoreLocked);
}

// The opening the restore and walkthrough traces share: the candidate and its Fedora package,
// the keep-awake holds, and any stale Windows run the preflight recovered.
function openGateTrace(options, gate, { runStart = {}, packageChecks } = {}) {
  const run = prepareHarnessRun(options);
  const evidencePath = evidencePathFor(gate, run.runId);
  const screenshotDir = join(options.linuxRoot, 'evidence', `${gate}-${run.runId}`);
  mkdirSync(dirname(evidencePath), { recursive: true });
  mkdirSync(screenshotDir, { recursive: true });
  observation(evidencePath, 'run-start', 'controller', { runId: run.runId, commit: run.candidateCommit, harnessCommit: run.harnessCommit, ...runStart });
  observation(evidencePath, 'package-evidence', 'fedora', packageChecks ? { ...run.fedoraPackage, checks: packageChecks() } : run.fedoraPackage);
  observation(evidencePath, 'keep-awake', 'controller', {
    fedora: 'systemd-inhibit sleep:idle; gnome-session-inhibit idle',
    windows: `PowerSetRequest SystemRequired, ${options.holdMinutes} min`,
  });
  if (run.recovered.recovered) observation(evidencePath, 'stale-run-recovered', 'windows', run.recovered);
  return { ...run, evidencePath, screenshotDir };
}

// Ends one machine's part of a gate, passed or failed: its processes stopped and its config
// restored. Returns the error when that fails.
async function finishMachine(machine, evidencePath) {
  try {
    const final = await machine?.cleanup(evidencePath);
    if (final) observation(evidencePath, 'process-final', machine.name, final);
    return undefined;
  } catch (error) {
    observation(evidencePath, 'cleanup-error', machine?.name ?? 'controller', { error: error.message });
    return error;
  }
}

async function runRestoreLocked(options) {
  const { candidateCommit, runId, evidencePath, screenshotDir } = openGateTrace(options, 'restore', {
    runStart: { killPoints: Object.keys(KILL_POINTS) },
  });

  let runError;
  let cleanupError;
  const machines = [
    () => linuxDriverMachine(options.linuxRoot, runId, randomUUID()),
    // Built before prepare runs, so a prepare that fails halfway is still finalized.
    () => windowsDriverMachine(options.sshHost, runId, randomUUID(), candidateCommit),
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
    cleanupError ??= await finishMachine(machine, evidencePath);
    if (runError || cleanupError) break;
  }

  if (runError) throw runError;
  if (cleanupError) throw cleanupError;
  observation(evidencePath, 'run-complete', 'controller');
  console.log(JSON.stringify({ runId, evidencePath, screenshotDir }));
}

// Enough of a ZIP reader for the diagnostic archive: stored and deflated members, no ZIP64.
export function zipEntries(buffer) {
  const end = buffer.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  if (end < 0) fail('diagnostic export is not a ZIP archive');
  const count = buffer.readUInt16LE(end + 10);
  let offset = buffer.readUInt32LE(end + 16);
  const entries = new Map();
  for (let index = 0; index < count; index += 1) {
    if (buffer.readUInt32LE(offset) !== 0x02014b50) fail('diagnostic archive has a damaged directory');
    const method = buffer.readUInt16LE(offset + 10);
    const compressedSize = buffer.readUInt32LE(offset + 20);
    const nameLength = buffer.readUInt16LE(offset + 28);
    const extraLength = buffer.readUInt16LE(offset + 30);
    const commentLength = buffer.readUInt16LE(offset + 32);
    const local = buffer.readUInt32LE(offset + 42);
    const name = buffer.toString('utf8', offset + 46, offset + 46 + nameLength);
    const dataStart = local + 30 + buffer.readUInt16LE(local + 26) + buffer.readUInt16LE(local + 28);
    const data = buffer.subarray(dataStart, dataStart + compressedSize);
    if (method !== 0 && method !== 8) fail(`diagnostic member ${name} uses compression ${method}`);
    entries.set(name, method === 8 ? inflateRawSync(data) : data);
    offset += 46 + nameLength + extraLength + commentLength;
  }
  return entries;
}

// Every planted value the default export must never contain, reported by member. A path is also
// sought JSON-escaped and with either separator, the forms a JSON member or log holds.
export function diagnosticLeaks(entries, planted) {
  const forms = (value) => [...new Set([value, value.replaceAll('\\', '/'), value.replaceAll('/', '\\')]
    .flatMap((form) => [form, JSON.stringify(form).slice(1, -1)]))];
  const leaks = [];
  for (const [name, data] of entries) {
    const text = data.toString('utf8');
    for (const [label, value] of Object.entries(planted)) {
      if (value && forms(value).some((form) => text.includes(form) || name.includes(form))) leaks.push({ member: name, planted: label });
    }
  }
  return leaks;
}

export async function ollamaEmbeddingModel(baseUrl) {
  const response = await fetch(`${baseUrl}/api/tags`, { signal: AbortSignal.timeout(10_000) }).catch((error) => {
    fail(`Ollama at ${baseUrl} is not answering (${error.message}); start it before the walkthrough`);
  });
  const { models = [] } = await response.json();
  if (!models.some((model) => model.name.startsWith('embeddinggemma'))) fail(`Ollama at ${baseUrl} has no embeddinggemma model`);
  return { embeddingModel: 'embeddinggemma' };
}

function fedoraSession() {
  const locked = runCommandSync('gdbus', [
    'call', '--session', '--dest', 'org.gnome.ScreenSaver', '--object-path', '/org/gnome/ScreenSaver',
    '--method', 'org.gnome.ScreenSaver.GetActive',
  ]).stdout;
  return { locked: locked.includes('true') };
}

// The desktop entry is what the GNOME launcher runs; it must name the installed binary.
function fedoraPackageChecks(rpmPath) {
  if (!existsSync(LINUX_DESKTOP_ENTRY)) fail(`installed desktop entry missing: ${LINUX_DESKTOP_ENTRY}`);
  const exec = /^Exec=(\S+)/m.exec(readFileSync(LINUX_DESKTOP_ENTRY, 'utf8'))?.[1];
  const resolved = exec?.startsWith('/') ? exec : runCommandSync('sh', ['-c', `command -v "${exec}"`]).stdout;
  if (resolved !== LINUX_APP) fail(`desktop entry runs ${exec}, not ${LINUX_APP}`);
  // rpm -V prints nothing when every installed file still matches the package.
  const verified = runCommandSync('rpm', ['-V', 'second-brain'], { accept: [0, 1] });
  if (verified.status !== 0) fail(`installed package files changed: ${verified.stdout}`);
  runCommandSync(join(REPO, 'scripts', 'verify-linux-package.sh'), [rpmPath], { timeout: 120_000 });
  const installed = runCommandSync('rpm', ['-q', '--qf', '%{SHA256HEADER}', 'second-brain']).stdout;
  const file = runCommandSync('rpm', ['-qp', '--qf', '%{SHA256HEADER}', rpmPath]).stdout;
  if (installed !== file) fail(`installed package is not ${rpmPath}`);
  return {
    desktopEntry: LINUX_DESKTOP_ENTRY,
    exec: resolved,
    rpmVerify: 'clean',
    rpm: { path: rpmPath, sha256: createHash('sha256').update(readFileSync(rpmPath)).digest('hex'), portalEntry: 'verified' },
  };
}

function fedoraOllamaTunnel(sshHost, windowsOllamaBaseUrl) {
  return spawn('ssh', [
    '-N', '-o', 'ExitOnForwardFailure=yes',
    '-L', `127.0.0.1:${FEDORA_OLLAMA_PORT}:${new URL(windowsOllamaBaseUrl).host}`, sshHost,
  ], { stdio: 'ignore' });
}

async function notionRequest(token, method, path, body) {
  const response = await fetch(`https://api.notion.com/v1${path}`, {
    method,
    headers: { Authorization: `Bearer ${token}`, 'Notion-Version': '2022-06-28', 'Content-Type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) fail(`Notion ${method} ${path} returned HTTP ${response.status}`);
  return response.json();
}

// Every result of a Notion list, following its cursor. A second copy past the first page
// must not escape the check (#186).
export async function notionAll(token, method, path, body = {}) {
  const results = [];
  let cursor = null;
  do {
    const page = method === 'GET'
      ? await notionRequest(token, 'GET', cursor ? `${path}&start_cursor=${cursor}` : path)
      : await notionRequest(token, method, path, cursor ? { ...body, start_cursor: cursor } : body);
    results.push(...page.results);
    cursor = page.has_more ? page.next_cursor : null;
  } while (cursor);
  return results;
}

function notionTitle(value) {
  return (value ?? []).map((part) => part.plain_text).join('');
}

/**
 * Proves the publish reached Notion, then archives the databases it went to. They are the ones
 * in the vault's own registry, so a row left under the disposable page by an earlier run cannot
 * count as this run's.
 */
// Each expected note must be in Notion exactly once. A second copy is the duplicate a lost
// connection could leave if the app ever created twice (#186).
export function publishedProblems(titles, published) {
  const missing = titles.filter((title) => !published.includes(title));
  const duplicated = titles.filter((title) => published.filter((found) => found === title).length > 1);
  return [
    ...(missing.length ? [`not published to Notion: ${missing.join(', ')}`] : []),
    ...(duplicated.length ? [`published to Notion more than once: ${duplicated.join(', ')}`] : []),
  ];
}

async function notionVerifyAndClean({ token }, registry, titles) {
  const databaseIds = Object.values(registry.databases).map((database) => database.database_id);
  if (!databaseIds.length) fail('the vault registered no Notion databases');
  const published = [];
  const problems = [];
  // A live database under the page that the registry does not name is a second one the app
  // made and lost track of (#186). It is archived with the rest and fails the run.
  let strays = [];
  if (registry.parent_page_id) {
    try {
      const children = await notionAll(token, 'GET', `/blocks/${registry.parent_page_id}/children?page_size=100`);
      strays = children
        .filter((block) => block.type === 'child_database' && !databaseIds.includes(block.id))
        .map((block) => block.id);
    } catch (error) {
      problems.push(error.message);
    }
  }
  for (const id of databaseIds) {
    try {
      const rows = await notionAll(token, 'POST', `/databases/${id}/query`, { page_size: 100 });
      for (const row of rows) {
        published.push(...Object.values(row.properties)
          .filter((property) => property.type === 'title').map((property) => notionTitle(property.title)));
      }
    } catch (error) {
      problems.push(error.message);
    }
  }
  // Archived even when a query failed, so the run leaves nothing live under the page.
  for (const id of [...databaseIds, ...strays]) {
    try {
      await notionRequest(token, 'PATCH', `/databases/${id}`, { archived: true });
    } catch (error) {
      problems.push(error.message);
    }
  }
  if (strays.length) problems.push(`${strays.length} database(s) under the page are not in the registry`);
  problems.push(...publishedProblems(titles, published));
  if (problems.length) fail(problems.join('; '));
  return { databases: databaseIds.length, archived: databaseIds.length + strays.length, published };
}

const NOTION_REGISTRY = '.helixnotes/notion/databases.json';

/**
 * Runs after every machine's walkthrough, passed or not: the run's token leaves the OS keyring
 * and its databases are archived. What was published is verified only when the walkthrough
 * passed; a token its Disconnect left behind fails that run.
 */
async function notionCleanup(machine, notion, passed) {
  // Every part runs and every problem is reported, so one failure cannot hide another.
  const problems = [];
  let token;
  try {
    token = machine.clearNotionToken();
    if (token.left) problems.push("the run's Notion token could not be removed from the keyring");
    if (passed && token.found) problems.push('Disconnect left the Notion token in the keyring');
  } catch (error) {
    problems.push(`keyring: ${error.message}`);
  }
  let registry;
  try {
    registry = JSON.parse(machine.readNote(NOTION_REGISTRY));
  } catch (error) {
    // A walkthrough that failed before Connect never created one.
    if (passed) problems.push(`registry: ${error.message}`);
  }
  const titles = passed ? NOTION_PUBLISHED_TITLES : [];
  let databases = { databases: 0, archived: 0 };
  try {
    if (registry) databases = await notionVerifyAndClean(notion, registry, titles);
  } catch (error) {
    problems.push(`databases: ${error.message}`);
  }
  const result = { ...databases, tokenFound: token?.found, tokenLeft: token?.left };
  if (problems.length) fail(`Notion cleanup: ${problems.join('; ')} ${JSON.stringify(result)}`);
  return result;
}

const WALKTHROUGH_NOTES = {
  semantic: {
    category: 'Areas',
    title: 'Sourdough starter',
    body: 'Feed the sourdough starter with equal parts flour and water every twelve hours. See [[Walkthrough tasks]].',
    query: 'how often should I feed my bread yeast culture',
  },
  tasks: { category: 'Resources', title: 'Walkthrough tasks', body: '- [ ] Walkthrough task to finish' },
  offline: { category: 'Areas', title: 'Offline capture', body: 'Captured while the embedding backend is offline.' },
};

const CLIP_URL = 'https://en.wikipedia.org/wiki/Zettelkasten';

// Every note in the vault when the walkthrough publishes, each expected in Notion exactly once.
// The clip and the attachment note carry anchor and relative links (#152).
export const NOTION_PUBLISHED_TITLES = [
  'Walkthrough capture', WALKTHROUGH_NOTES.semantic.title, WALKTHROUGH_NOTES.tasks.title,
  'Zettelkasten', 'Walkthrough attachment',
];

async function walkthroughGate(machine, { vault, evidencePath, screenshotDir, notion, requireUnlocked }) {
  const walkthrough = await import('./alpha-walkthrough.mjs');
  const type = walkthrough.typist(machine.name === 'windows');
  const record = (event, value) => observation(evidencePath, event, machine.name, value);
  let stepNumber = 0;
  const sessionState = () => {
    const state = machine.session();
    if (state.locked || (requireUnlocked && state.desktopSession == null)) {
      fail(`${machine.name} desktop is ${state.locked ? 'locked' : 'not signed in'}`);
    }
    return state;
  };
  // One screenshot per step, taken whether the step passed or failed. A step without a browser
  // gets its screenshot from the action. The session is read as the step starts: the Windows
  // desktop has to be unlocked for the step to run.
  const step = async (browser, name, action) => {
    stepNumber += 1;
    const screenshot = join(screenshotDir, `${machine.name}-${String(stepNumber).padStart(2, '0')}-${name}.png`);
    const session = sessionState();
    try {
      const result = await action(screenshot);
      await browser?.saveScreenshot(screenshot);
      record('step', { step: name, result, screenshot, session });
      return result;
    } catch (error) {
      await browser?.saveScreenshot(screenshot).catch(() => {});
      record('step-failed', { step: name, error: error.message, screenshot: existsSync(screenshot) ? screenshot : null });
      throw error;
    }
  };

  // Deleting the WebDriver session kills the app, so the window's own close has to finish
  // first: its save-aware shutdown is what row A of the matrix measures.
  const closeAndWait = async (browser) => {
    await walkthrough.closeWindow(browser);
    return machine.appsGone();
  };

  record('session', sessionState());
  record('entry-launched', await machine.launchEntry());
  record('driver-started', await machine.startDriver());
  const planted = { secret: `sk-alpha-harness-${randomUUID()}` };
  record('config-patched', { keys: machine.patchConfig({ openai_api_key: planted.secret }) });

  let capture;
  await withApp(machine, async (browser) => {
    await step(browser, 'vault-open', () => walkthrough.vaultOpened(browser));
    await step(browser, 'sync-running', () => machine.syncRunning());
    capture = await step(browser, 'save-lifecycle', () => walkthrough.saveLifecycle(browser, type));
    await closeAndWait(browser);
  });
  const saved = machine.readNote(capture.relativePath);
  const missing = capture.expected.filter((text) => !saved.includes(text));
  record('saves-survived-close', { note: capture.relativePath, missing });
  if (missing.length) fail(`edits lost across navigation and close: ${JSON.stringify(missing)}`);

  const { semantic, tasks } = WALKTHROUGH_NOTES;
  await withApp(machine, async (browser) => {
    await step(browser, 'fixture-notes', async () => {
      await walkthrough.createNote(browser, type, semantic);
      await walkthrough.createNote(browser, type, tasks);
      return { notes: [semantic.title, tasks.title] };
    });
    await step(browser, 'keyword-search', () => walkthrough.search(browser, type, { mode: 'Keyword', query: 'sourdough', expected: semantic.title }));
    await step(browser, 'semantic-index', () => walkthrough.waitForSemanticIndex(browser));
    await step(browser, 'semantic-search', () => walkthrough.search(browser, type, { mode: 'Semantic', query: semantic.query, expected: semantic.title }));
    await step(browser, 'graph', () => walkthrough.graph(browser));
    await walkthrough.closeGraph(browser);
    await step(browser, 'tasks', () => walkthrough.tasks(browser, { text: 'Walkthrough task' }));
    await step(browser, 'trash-restore', () => walkthrough.trashAndRestore(browser, semantic));
    await step(browser, 'note-history', () => walkthrough.noteHistory(browser, type, semantic));
    await step(browser, 'backup', () => walkthrough.backupNow(browser));
    await walkthrough.closeSettings(browser);
    await step(browser, 'restore', () => walkthrough.restoreLatest(browser));
    await walkthrough.closeSettings(browser);
    await step(browser, 'web-clip', () => walkthrough.clipPage(browser, type, { url: CLIP_URL, category: 'Resources', expectedText: 'Zettelkasten' }));
    // Its own note, not the clip: a reload of the freshly clipped note cancels the insert.
    await step(browser, 'attach-file', async () => {
      await walkthrough.createNote(browser, type, { category: 'Resources', title: 'Walkthrough attachment' });
      const attachment = { name: 'walkthrough-attachment.txt', content: 'Attached by the alpha walkthrough.' };
      return walkthrough.attachFile(browser, { ...attachment, path: machine.stageFile(attachment.name, attachment.content) });
    });
    await step(browser, 'notion-publish', () => walkthrough.notionPublish(browser, type, notion, () => {
      const stored = machine.notionTokenStored();
      if (!stored) fail("the keyring lookup does not find the connected vault's Notion token");
      return stored;
    }));
    await step(browser, 'diagnostics-export', () => walkthrough.exportDiagnostics(browser, machine.diagnosticsPath));
    await walkthrough.closeSettings(browser);
    await closeAndWait(browser);
  });

  const archive = zipEntries(machine.fetchDiagnostics(join(screenshotDir, `${machine.name}-diagnostics.zip`)));
  const leaks = diagnosticLeaks(archive, {
    ...planted,
    body: walkthrough.MARKER,
    title: capture.title,
    path: capture.relativePath,
    vault,
  });
  record('diagnostics-checked', { members: [...archive.keys()], leaks });
  if (leaks.length) fail(`diagnostic export leaked planted values: ${JSON.stringify(leaks)}`);

  // AI offline: capture, editing, organization, and keyword search keep working.
  record('config-patched', { keys: machine.patchConfig({ ollama_base_url: OFFLINE_OLLAMA_URL }) });
  const { offline } = WALKTHROUGH_NOTES;
  const offlineEdit = ' Edited while the embedding backend is offline.';
  await withApp(machine, async (browser) => {
    await step(browser, 'offline-capture', () => walkthrough.createNote(browser, type, offline).then(() => offline));
    await step(browser, 'offline-edit', () => walkthrough.editNote(browser, type, { category: capture.category, title: capture.title, text: offlineEdit }));
    await step(browser, 'offline-organize', () => walkthrough.moveNote(browser, { from: offline.category, to: 'Archives', title: offline.title }));
    await step(browser, 'offline-keyword-search', () => walkthrough.search(browser, type, { mode: 'Keyword', query: 'offline', expected: offline.title }));
    await closeAndWait(browser);
  });
  const offlineSaved = machine.readNote(capture.relativePath).includes(offlineEdit.trim());
  record('offline-edit-saved', { note: capture.relativePath, saved: offlineSaved });
  if (!offlineSaved) fail('the offline edit was not on disk after the app exited');

  record('config-broken', machine.breakConfig() ?? {});
  await withApp(machine, async (browser) => {
    await step(browser, 'malformed-config', () => walkthrough.startupError(browser));
  }, '.error');
  const repaired = machine.repairConfig();
  record('config-repaired', repaired);
  // The app keeps the file it could not parse; this run's is the only one it may have left.
  if (repaired.damaged.length !== 1) fail(`expected one damaged config kept, found ${JSON.stringify(repaired.damaged)}`);

  if (machine.hotkeyCheck) await step(null, 'renamed-vault-hotkey', (screenshot) => machine.hotkeyCheck(screenshot));

  // The last exit is a normal one, through the close button, with sync running.
  await withApp(machine, async (browser) => {
    await step(browser, 'final-launch', () => machine.syncRunning());
    record('normal-exit', await closeAndWait(browser));
  });
  record('process-final', await machine.appsGone());
}

async function runWalkthrough(args) {
  // Checked before runOnFedora takes the machines, so a missing input costs nothing.
  const options = parseOptions(args);
  if (!options.windowsInstaller) fail('--windows-installer names the candidate NSIS setup on the Windows machine');
  if (!options.fedoraRpm) fail('--fedora-rpm names the installed candidate RPM, which verify-linux-package.sh checks');
  notionCredentials();
  return runOnFedora(args, runWalkthroughLocked);
}

function notionCredentials() {
  const token = process.env.SECOND_BRAIN_NOTION_TOKEN;
  const page = process.env.SECOND_BRAIN_NOTION_PAGE;
  if (!token || !page) fail('set SECOND_BRAIN_NOTION_TOKEN and SECOND_BRAIN_NOTION_PAGE for the disposable Notion workspace');
  return { token, page };
}

async function runWalkthroughLocked(options) {
  const notion = notionCredentials();
  const windowsOllamaBaseUrl = windowsOllamaUrl(options.ollamaPort);
  // `--machine` runs one machine while a step is being fixed; only a run of both is the gate.
  const machineNames = options.machine ? [options.machine] : ['fedora', 'windows'];
  if (!machineNames.every((name) => ['fedora', 'windows'].includes(name))) fail(`--machine must be fedora or windows: ${options.machine}`);
  const { candidateCommit, runId, evidencePath, screenshotDir } = openGateTrace(options, 'walkthrough', {
    runStart: { clipUrl: CLIP_URL, machines: machineNames, gate: machineNames.length === 2 },
    packageChecks: () => fedoraPackageChecks(options.fedoraRpm),
  });
  const controller = (event, value) => observation(evidencePath, event, 'controller', value);

  const ollamaTunnel = fedoraOllamaTunnel(options.sshHost, windowsOllamaBaseUrl);
  let runError;
  let cleanupError;
  try {
    await sleep(2_000);
    if (ollamaTunnel.exitCode !== null) fail(`Ollama tunnel exited ${ollamaTunnel.exitCode}; is port ${FEDORA_OLLAMA_PORT} taken?`);
    controller('ollama', {
      windows: remoteWorker(options.sshHost, { action: 'ollama', root: WINDOWS_ROOT, runId: 'preflight', ollamaPort: options.ollamaPort }),
      fedora: await ollamaEmbeddingModel(`http://127.0.0.1:${FEDORA_OLLAMA_PORT}`),
    });
    const machines = [
      ['fedora', () => linuxDriverMachine(options.linuxRoot, runId, randomUUID(), { ollamaBaseUrl: `http://127.0.0.1:${FEDORA_OLLAMA_PORT}`, sync: true })],
      ['windows', () => windowsDriverMachine(options.sshHost, runId, randomUUID(), candidateCommit, { ollamaBaseUrl: windowsOllamaBaseUrl, sync: true })],
    ].filter(([name]) => machineNames.includes(name)).map(([, make]) => make);
    for (const makeMachine of machines) {
      let machine;
      let passed = false;
      try {
        machine = makeMachine();
        if (machine.install) observation(evidencePath, 'installed', machine.name, machine.install(options.windowsInstaller));
        const prepared = machine.prepare();
        observation(evidencePath, 'prepared', machine.name, prepared);
        await walkthroughGate(machine, {
          vault: prepared.vault, evidencePath, screenshotDir, notion,
          // #137: the Windows walkthrough must run on an unlocked interactive desktop.
          requireUnlocked: machine.name === 'windows',
        });
        passed = true;
      } catch (error) {
        runError = error;
        observation(evidencePath, 'run-failed', machine?.name ?? 'controller', { error: error.message });
      }
      if (machine) {
        try {
          observation(evidencePath, 'notion-checked', machine.name, await notionCleanup(machine, notion, passed));
        } catch (error) {
          observation(evidencePath, 'run-failed', machine.name, { error: error.message });
          runError ??= error;
        }
      }
      cleanupError ??= await finishMachine(machine, evidencePath);
      if (!runError && !cleanupError) {
        // The uninstall check compares against this; for the RPM that happens after the run.
        const vaultBefore = machine.hash();
        observation(evidencePath, 'vault-final', machine.name, vaultBefore);
        if (machine.uninstall) {
          try {
            const removed = { ...machine.uninstall(), vaultUnchanged: machine.hash().sha256 === vaultBefore.sha256 };
            observation(evidencePath, 'uninstalled', machine.name, removed);
            assertUninstalled(machine.name, removed);
          } catch (error) {
            runError = error;
            observation(evidencePath, 'run-failed', machine.name, { error: error.message });
          } finally {
            // Leave the machine with the candidate installed, as the run found it. A failure here
            // is recorded without hiding one from the uninstall.
            try {
              observation(evidencePath, 'reinstalled', machine.name, machine.install(options.windowsInstaller));
            } catch (error) {
              observation(evidencePath, 'run-failed', machine.name, { error: `reinstall: ${error.message}` });
              runError ??= error;
            }
          }
        }
      }
      if (runError || cleanupError) break;
    }
    if (!runError && !cleanupError) {
      try {
        controller('ollama-final', {
          windows: remoteWorker(options.sshHost, { action: 'ollama', root: WINDOWS_ROOT, runId: 'final', ollamaPort: options.ollamaPort }),
          fedora: await ollamaEmbeddingModel(`http://127.0.0.1:${FEDORA_OLLAMA_PORT}`),
        });
      } catch (error) {
        runError = error;
        controller('run-failed', { error: error.message });
      }
    }
  } finally {
    ollamaTunnel.kill();
  }

  if (runError) throw runError;
  if (cleanupError) throw cleanupError;
  controller('run-complete', {
    machines: machineNames,
    gate: machineNames.length === 2,
    fedoraUninstall: machineNames.includes('fedora')
      ? `pending: run "alpha-harness.mjs walkthrough-uninstalled --run ${runId}" after rpm -e`
      : 'not run',
  });
  console.log(JSON.stringify({ runId, evidencePath, screenshotDir }));
}

// The RPM needs root to remove, so Nicolas runs `sudo rpm -e second-brain` after the walkthrough
// and this checks the result: the package is gone and the walkthrough's vault still has the hash
// the run recorded after the app exited.
function runWalkthroughUninstalled(args) {
  const options = parseOptions(args);
  if (!options.runId) fail('--run names the walkthrough run to check');
  const evidencePath = evidencePathFor('walkthrough', options.runId);
  if (!existsSync(evidencePath)) fail(`no walkthrough evidence for run ${options.runId}`);
  const trace = readFileSync(evidencePath, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
  const complete = trace.find((line) => line.event === 'run-complete');
  if (!complete?.gate) fail(`run ${options.runId} is not a completed walkthrough of both machines`);
  if (trace.some((line) => line.event === 'uninstalled' && line.machine === 'fedora')) {
    fail(`run ${options.runId} already has its Fedora uninstall result`);
  }
  const before = trace.find((line) => line.event === 'vault-final' && line.machine === 'fedora');
  const vault = join(options.linuxRoot, 'runs', options.runId, 'fedora', 'vault');
  const query = runCommandSync('rpm', ['-q', 'second-brain'], { accept: [0, 1] });
  const result = {
    packageRemoved: query.status === 1,
    appRemoved: !existsSync(LINUX_APP),
    desktopEntryRemoved: !existsSync(LINUX_DESKTOP_ENTRY),
    vaultUnchanged: treeHash(vault).sha256 === before.sha256,
  };
  observation(evidencePath, 'uninstalled', 'fedora', result);
  assertUninstalled('fedora', result);
  console.log(JSON.stringify(result));
}

// Every field of an uninstall result is a check that has to hold.
function assertUninstalled(machineName, result) {
  if (Object.values(result).some((held) => held !== true)) {
    fail(`${machineName} uninstall did not leave the expected state: ${JSON.stringify(result)}`);
  }
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

// Runs alpha-desktop.ps1 in the interactive desktop session through a scheduled task.
function runDesktopScript(tools, scriptArguments) {
  const ran = windowsPowerShell(tools, [
    '-Action', 'DesktopScript', '-TaskName', WINDOWS_DESKTOP_TASK,
    '-ExecutablePath', win32.join(tools, 'alpha-desktop.ps1'), '-ScriptArguments', scriptArguments,
  ]);
  if (ran.result !== 0) fail(`desktop script failed with ${ran.result}: ${scriptArguments}`);
  return ran;
}

// alpha-desktop.ps1 writes JSON with Set-Content, which adds a UTF-8 byte-order mark.
function readDesktopJson(path) {
  return JSON.parse(readFileSync(path, 'utf8').replace(/^\uFEFF/, ''));
}

function damagedConfigs(configDirectory) {
  return readdirSync(configDirectory).filter((name) => name.startsWith(DAMAGED_CONFIG_PREFIX));
}

// tauri-plugin-window-state saves each launch's window geometry beside config.json, so a run
// journals that file too. A null original records that the file did not exist before the run.
export function journalWindowState(windowStatePath, originalPath) {
  if (!existsSync(windowStatePath)) return { windowStatePath, originalWindowStatePath: null, originalWindowStateSha256: null };
  const bytes = readFileSync(windowStatePath);
  writeFileSync(originalPath, bytes);
  return {
    windowStatePath,
    originalWindowStatePath: originalPath,
    originalWindowStateSha256: createHash('sha256').update(bytes).digest('hex'),
  };
}

// Every launch writes these profile folders, and nothing else redirects them (#174). A run
// renames each folder aside and puts a junction to the run directory in its place, so the
// profile ends the run as it began and the run's logs stay with its evidence.
export const PROFILE_REDIRECTS = ['logs', 'EBWebView'];

// Only a missing path is absent. Any other failure to look, such as a denied access, is thrown:
// read as absent, it would let a restore report a folder back while it is still aside.
function present(path) {
  try {
    lstatSync(path);
    return true;
  } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw error;
  }
}

function linksTo(path, target) {
  return resolve(readlinkSync(path)).toLowerCase() === resolve(target).toLowerCase();
}

// Run before prepare changes anything, so a profile the run must not touch fails the run
// without leaving a lock, a junction, or a run directory behind.
export function checkProfileRedirects(redirects) {
  for (const { path, aside } of redirects) {
    if (present(path) && lstatSync(path).isSymbolicLink()) fail(`${path} is already redirected`);
    if (present(aside)) fail(`${aside} already exists`);
  }
}

// `link(target, path)` creates the link: a junction on Windows, a symlink in the tests.
export function redirectProfileFolders(redirects, link) {
  checkProfileRedirects(redirects);
  for (const { path, target, aside } of redirects) {
    mkdirSync(target, { recursive: true });
    if (present(path)) {
      mkdirSync(dirname(aside), { recursive: true });
      renameSync(path, aside);
    }
    link(target, path);
  }
}

// Idempotent, so recover and finalize can finish a run stopped at any point of either
// direction. Only a link to this run's own target is removed. rmdir removes a Windows junction
// and never its target; a POSIX symlink (the tests) needs unlink. The user's folder is renamed
// back, never copied or deleted.
export function restoreProfileFolders(redirects = []) {
  const removeLink = process.platform === 'win32' ? rmdirSync : unlinkSync;
  return redirects.map(({ path, target, aside }) => {
    if (present(path) && lstatSync(path).isSymbolicLink()) {
      if (!linksTo(path, target)) fail(`refusing to remove ${path}: not this run's junction`);
      removeLink(path);
    }
    const hadFolder = present(aside);
    if (hadFolder) {
      if (present(path)) fail(`refusing to restore ${aside}: ${path} exists again`);
      renameSync(aside, path);
    }
    // Also when nothing was aside, so a restore stopped right after the rename still clears it.
    try {
      rmdirSync(dirname(aside));
    } catch {
      // Another redirect's folder is still aside, or it is already gone; only an empty
      // directory is ever removed.
    }
    // Without a folder aside, the path is either absent as before the run or the user's own
    // folder, because the run stopped before redirecting it; both are left as they are.
    return { path, movedBack: hadFolder };
  });
}

// Journals written before #167 hold no window state, so there is nothing to restore for them.
export function restoreWindowState(manifest) {
  if (!manifest.windowStatePath) return;
  if (manifest.originalWindowStatePath) atomicWrite(manifest.windowStatePath, readFileSync(manifest.originalWindowStatePath));
  else rmSync(manifest.windowStatePath, { force: true });
}

// Compares the live file with the hash taken before the run, not with the journal copy, so a
// damaged copy fails too. Returns that hash, or null when the file was absent before and after or
// the journal predates #167.
export function checkWindowStateRestored(manifest) {
  if (!manifest.windowStatePath) return null;
  const present = existsSync(manifest.windowStatePath);
  if (!manifest.originalWindowStatePath) {
    if (present) fail(`${manifest.windowStatePath} did not exist before the run but exists now`);
    return null;
  }
  const current = present ? createHash('sha256').update(readFileSync(manifest.windowStatePath)).digest('hex') : null;
  if (current !== manifest.originalWindowStateSha256) fail(`${manifest.windowStatePath} does not match its pre-run hash`);
  return current;
}

// Checks the restored window state and returns its evidence; the flag tells an absent file from a
// journal before #167.
function checkedWindowStateEvidence(manifest) {
  return { windowStateJournaled: Boolean(manifest.windowStatePath), windowStateSha256: checkWindowStateRestored(manifest) };
}

// Restores config.json and the window state from the journal, checks the window state, and
// returns its evidence.
function restoreJournaledFiles(manifest) {
  atomicWrite(manifest.configPath, readFileSync(manifest.originalPath));
  restoreWindowState(manifest);
  return checkedWindowStateEvidence(manifest);
}

function damagedBeforePath(paths) {
  return win32.join(paths.runRoot, 'damaged-before-break.json');
}

// The app keeps a config it could not parse beside the new one. Those from before the run broke
// the config are the user's, so only the run's own leaves the profile. The profile is on C: and
// the run root on D:, where a rename fails with EXDEV, so it is copied and deleted.
function moveRunDamagedConfigs(configDirectory, paths) {
  if (!existsSync(damagedBeforePath(paths))) return [];
  const before = JSON.parse(readFileSync(damagedBeforePath(paths), 'utf8'));
  const damaged = damagedConfigs(configDirectory).filter((name) => !before.includes(name));
  for (const name of damaged) {
    copyFileSync(win32.join(configDirectory, name), win32.join(paths.runRoot, name));
    unlinkSync(win32.join(configDirectory, name));
  }
  return damaged;
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
    const configPath = win32.join(process.env.APPDATA, APP_IDENTIFIER, 'config.json');
    const lockPath = win32.join(dirname(configPath), 'alpha-harness.lock.json');
    if (!existsSync(lockPath)) return { recovered: false };
    const report = windowsInterrupt(tools, appPath, 'Report');
    if (report.processes.length) fail('stale harness lock exists while installed processes are running');
    const lock = JSON.parse(readFileSync(lockPath, 'utf8'));
    const manifest = JSON.parse(readFileSync(lock.manifestPath, 'utf8'));
    if (manifest.runId !== lock.runId || win32.resolve(manifest.configPath) !== win32.resolve(configPath)) {
      fail('stale harness journal is inconsistent');
    }
    const windowState = restoreJournaledFiles(manifest);
    const profile = restoreProfileFolders(manifest.profileRedirects);
    unlinkSync(lockPath);
    return { recovered: true, runId: manifest.runId, runRoot: dirname(lock.manifestPath), ...windowState, profile };
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
    // Checked before anything below touches the disk, so a refused profile leaves no trace.
    const profileRedirects = PROFILE_REDIRECTS.map((name) => ({
      path: win32.join(process.env.LOCALAPPDATA, APP_IDENTIFIER, name),
      target: win32.join(paths.runRoot, 'profile', name),
      aside: win32.join(process.env.LOCALAPPDATA, `${APP_IDENTIFIER}.alpha-harness-${request.runId}`, name),
    }));
    checkProfileRedirects(profileRedirects);
    mkdirSync(paths.backupPath, { recursive: true });
    mkdirSync(paths.machinePath, { recursive: true });
    makeVault(paths.vaultPath, request.vaultId);

    const configPath = win32.join(process.env.APPDATA, APP_IDENTIFIER, 'config.json');
    if (!existsSync(configPath)) fail(`installed app config is missing: ${configPath}`);
    const originalPath = win32.join(paths.runRoot, 'original-config.json');
    copyFileSync(configPath, originalPath);
    const windowStateJournal = journalWindowState(
      win32.join(dirname(configPath), '.window-state.json'), win32.join(paths.runRoot, 'original-window-state.json'),
    );
    const original = JSON.parse(readFileSync(configPath, 'utf8'));
    const localVaults = win32.join(process.env.LOCALAPPDATA, APP_IDENTIFIER, 'vaults');
    const machineLink = win32.join(localVaults, request.vaultId);
    mkdirSync(localVaults, { recursive: true });
    if (existsSync(machineLink)) fail(`machine-state path already exists: ${machineLink}`);
    mkdirSync(dirname(machineLink), { recursive: true });
    const junction = runCommandSync('cmd.exe', ['/d', '/s', '/c', 'mklink', '/J', machineLink, paths.machinePath]);
    if (!junction.stdout) fail('could not create machine-state junction');

    if (!request.driven || request.sync) {
      const control = makeControl(request.vaultId);
      atomicWrite(win32.join(paths.machinePath, 'sync-control.json'), `${JSON.stringify(control, null, 2)}\n`);
    }
    const lockPath = win32.join(dirname(configPath), 'alpha-harness.lock.json');
    const manifest = {
      version: 1, runId: request.runId, startedAt: new Date().toISOString(), configPath, originalPath, ...windowStateJournal, machineLink, appPath, profileRedirects,
    };
    atomicWrite(paths.manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    writeFileSync(lockPath, `${JSON.stringify({ runId: request.runId, manifestPath: paths.manifestPath })}\n`, { flag: 'wx' });
    // After the lock, so a run stopped partway through is finished by the next run's recover.
    redirectProfileFolders(profileRedirects, (target, path) => {
      if (!runCommandSync('cmd.exe', ['/d', '/s', '/c', 'mklink', '/J', path, target]).stdout) fail(`could not redirect ${path}`);
    });
    atomicWrite(
      configPath,
      `${JSON.stringify(harnessConfig(original, paths.vaultPath, request.vaultId, paths.backupPath, { driven: request.driven, ollamaBaseUrl: request.ollamaBaseUrl }), null, 2)}\n`,
    );
    return {
      runRoot: paths.runRoot,
      vault: paths.vaultPath,
      freeBytes: requireFreeSpace(root),
      package: { ...system, app },
      windowStateSha256: windowStateJournal.originalWindowStateSha256,
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

  if (request.action === 'launch-entry') {
    if (windowsInterrupt(tools, appPath, 'Report').processes.length) fail('installed app is already running');
    const resultPath = win32.join(paths.runRoot, 'entry-launch.json');
    runDesktopScript(tools, `-Mode LaunchShortcut -OutputPath "${resultPath}"`);
    const { shortcut } = readDesktopJson(resultPath);
    unlinkSync(resultPath);
    const report = await waitForState(
      () => windowsInterrupt(tools, appPath, 'Report'),
      (state) => state.processes.filter((row) => row.Role === 'App').length === 1,
      30_000, 'Windows Start-menu entry launch',
    );
    const app = report.processes.find((row) => row.Role === 'App');
    try {
      await sleep(2_000);
      if (!windowsInterrupt(tools, appPath, 'Report').processes.some((row) => row.Id === app.Id && row.Role === 'App')) {
        fail('Windows Start-menu entry app exited after launch');
      }
      return { entry: shortcut, executable: app.Path, pid: app.Id };
    } finally {
      windowsPowerShell(tools, ['-Action', 'StopPid', '-PidToStop', String(app.Id), '-ExecutablePath', appPath]);
      await waitForState(
        () => windowsInterrupt(tools, appPath, 'Report').processes,
        (processes) => processes.length === 0, 60_000, 'Windows processes after Start-menu entry exit',
      );
    }
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
    snapshotVault(paths.vaultPath, paths.snapshotPath);
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

  if (request.action === 'read-note') {
    const notePath = win32.join(paths.vaultPath, request.relativePath);
    assertWindowsRoot(paths.vaultPath, notePath);
    return { content: readFileSync(notePath, 'utf8') };
  }

  if (['patch-config', 'break-config', 'repair-config'].includes(request.action)) {
    // Only the config this run swapped in, while this run still holds the lock on it.
    const manifest = JSON.parse(readFileSync(paths.manifestPath, 'utf8'));
    const lock = JSON.parse(readFileSync(win32.join(dirname(manifest.configPath), 'alpha-harness.lock.json'), 'utf8'));
    if (manifest.runId !== request.runId || lock.runId !== request.runId) fail('config lock belongs to another run');
    const goodConfigPath = win32.join(paths.runRoot, GOOD_CONFIG_NAME);
    if (request.action === 'patch-config') return { patched: patchConfigFile(manifest.configPath, request.patch) };
    if (request.action === 'break-config') {
      writeFileSync(damagedBeforePath(paths), JSON.stringify(damagedConfigs(dirname(manifest.configPath))));
      copyFileSync(manifest.configPath, goodConfigPath);
      atomicWrite(manifest.configPath, MALFORMED_CONFIG);
      return { broken: manifest.configPath };
    }
    const damaged = moveRunDamagedConfigs(dirname(manifest.configPath), paths);
    atomicWrite(manifest.configPath, readFileSync(goodConfigPath));
    return { damaged };
  }

  if (request.action === 'ollama') return ollamaEmbeddingModel(windowsOllamaUrl(request.ollamaPort));

  if (request.action === 'install') {
    const installer = win32.resolve(request.installer);
    assertWindowsRoot('D:\\SecondBrainTest', installer);
    if (!existsSync(installer)) fail(`installer missing: ${installer}`);
    if (existsSync(appPath) && windowsInterrupt(tools, appPath, 'Report').processes.length) {
      fail('refusing to install while the app runs');
    }
    const installed = spawnSync(installer, ['/S', `/D=${dirname(appPath)}`], { windowsHide: true, timeout: 5 * 60_000 });
    if (installed.error || installed.status !== 0) fail(`installer exited ${installed.status}: ${installed.error?.message ?? ''}`);
    const shortcut = windowsPowerShell(tools, ['-Action', 'Shortcut', '-ExecutablePath', appPath]);
    if (!shortcut.exists || !shortcut.matches) fail(`Start-menu entry does not run ${appPath}: ${JSON.stringify(shortcut)}`);
    return {
      installer: { path: installer, sha256: createHash('sha256').update(readFileSync(installer)).digest('hex') },
      app: binaryEvidence(appPath, request.candidateCommit),
      shortcut,
    };
  }

  if (request.action === 'uninstall') {
    if (windowsInterrupt(tools, appPath, 'Report').processes.length) fail('refusing to uninstall while the app runs');
    const uninstaller = win32.join(dirname(appPath), 'uninstall.exe');
    // The NSIS uninstaller copies itself to %TEMP% and returns before the files are gone.
    const started = spawnSync(uninstaller, ['/S'], { windowsHide: true, timeout: 60_000 });
    if (started.error || started.status !== 0) fail(`uninstaller exited ${started.status}: ${started.error?.message ?? ''}`);
    const deadline = Date.now() + 2 * 60_000;
    while (existsSync(appPath) && Date.now() < deadline) await sleep(500);
    return {
      appRemoved: !existsSync(appPath),
      shortcutRemoved: !windowsPowerShell(tools, ['-Action', 'Shortcut', '-ExecutablePath', appPath]).exists,
    };
  }

  if (request.action === 'notion-token') {
    const resultPath = win32.join(paths.runRoot, 'notion-token.json');
    // keyring's Windows store names a credential "<username>.<service>".
    const target = `integration:notion:${request.vaultId}.${APP_IDENTIFIER}`;
    runDesktopScript(tools, `-Mode NotionToken -OutputPath "${resultPath}" -CredentialTarget "${target}"${request.remove ? ' -RemoveCredential' : ''}`);
    const found = readDesktopJson(resultPath);
    unlinkSync(resultPath);
    return found;
  }

  if (request.action === 'stage-file') {
    const staged = win32.join(paths.runRoot, request.name);
    assertWindowsRoot(paths.runRoot, staged);
    writeFileSync(staged, request.base64 ? Buffer.from(request.base64, 'base64') : request.content);
    return { path: staged };
  }

  // #28 acceptance gate reads. Only reads, except the ignore list the delayed-attachment check
  // sets on this machine's own folder.
  if (request.action === 'syncthing') {
    const writable = request.method === 'POST' && request.path.startsWith('/rest/db/ignores?');
    if (request.method !== 'GET' && !writable) fail(`refusing Syncthing ${request.method} ${request.path}`);
    return { value: await syncthingRequest(windowsControl(paths), request.method, request.path, request.body) };
  }
  if (request.action === 'vault-summary') return vaultSummary(paths.vaultPath, request.markers);
  if (request.action === 'backup-summary') {
    const archive = win32.join(paths.backupPath, request.name);
    assertWindowsRoot(paths.backupPath, archive);
    return backupSummary(archive, request.wanted, request.markers);
  }
  if (request.action === 'isolation') {
    const script = win32.join(tools, 'app-isolation.ps1');
    const args = ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', script, '-RunId', request.runId];
    if (request.mode === 'isolate') {
      // The app's own programs, and the WebView2 runtime its window runs in.
      const webview = JSON.parse(runCommandSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
        "@(Get-CimInstance Win32_Process -Filter \"Name = 'msedgewebview2.exe'\" | ForEach-Object ExecutablePath | Where-Object { $_ } | Sort-Object -Unique) | ConvertTo-Json -Compress"]).stdout || '[]');
      const programs = [appPath, win32.join(dirname(appPath), 'syncthing.exe'), ...[webview].flat()];
      return { programs, ...parseLastJson(runCommandSync('powershell.exe', [...args, '-Action', 'Isolate', '-ProgramPath', programs.join('|'), '-DeadlineMinutes', '20']).stdout) };
    }
    return parseLastJson(runCommandSync('powershell.exe', [...args, '-Action', request.mode === 'restore' ? 'Restore' : 'State']).stdout);
  }
  if (request.action === 'connections') {
    // Every TCP connection the app, its sidecar, and its WebView2 processes hold right now.
    const report = windowsInterrupt(tools, appPath, 'Report');
    const pids = report.processes.map((process) => process.Id);
    if (!pids.length) return { pids, connections: [] };
    const script = `$ids = @(${pids.join(',')}); $all = @(Get-CimInstance Win32_Process); `
      + '$kids = @($all | Where-Object { $ids -contains $_.ParentProcessId -and $_.Name -eq \'msedgewebview2.exe\' } | ForEach-Object ProcessId); '
      + '$ids += $kids; $ids += @($all | Where-Object { $kids -contains $_.ParentProcessId } | ForEach-Object ProcessId); '
      + '@(Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object { $ids -contains $_.OwningProcess -and $_.State -ne \'Listen\' } | '
      + 'ForEach-Object { [pscustomobject]@{ pid = $_.OwningProcess; remote = $_.RemoteAddress; port = $_.RemotePort; state = [string]$_.State } }) | ConvertTo-Json -Compress';
    const connections = [JSON.parse(runCommandSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script]).stdout || '[]')].flat();
    return { pids, connections };
  }

  if (request.action === 'rename-vault') {
    const moved = `${paths.vaultPath}-renamed`;
    const [from, to] = request.away ? [paths.vaultPath, moved] : [moved, paths.vaultPath];
    renameSync(from, to);
    return { from, to };
  }

  if (request.action === 'desktop') {
    const image = win32.join(paths.runRoot, request.image);
    assertWindowsRoot(paths.runRoot, image);
    // The app logs each vault-unavailable toast it shows (src-tauri/src/hotkey/windows.rs). The
    // profile's logs folder leads into this run's directory, so the lines are this run's.
    const logDir = win32.join(process.env.LOCALAPPDATA, APP_IDENTIFIER, 'logs');
    const countToastLogLines = () => readdirSync(logDir).filter((name) => name.endsWith('.log')).reduce((count, name) => (
      count + readFileSync(win32.join(logDir, name), 'utf8').split('Showed the vault-unavailable notification').length - 1
    ), 0);
    const toastsShownBefore = countToastLogLines();
    const ran = runDesktopScript(tools, `-Mode ${request.mode} -OutputPath "${image}" -Aumid ${APP_IDENTIFIER}`);
    if (!existsSync(image)) fail(`desktop script wrote no screenshot: ${image}`);
    return { image, ...ran, toastsShown: countToastLogLines() - toastsShownBefore, toasts: readDesktopJson(image.replace(/\.png$/, '.json')) };
  }

  if (request.action === 'session') return windowsPowerShell(tools, ['-Action', 'SessionState']);

  if (request.action === 'finalize') {
    const report = windowsInterrupt(tools, appPath, 'Report');
    if (report.processes.length) fail('refusing to restore config while installed processes remain');
    const manifest = JSON.parse(readFileSync(paths.manifestPath, 'utf8'));
    if (manifest.runId !== request.runId || win32.resolve(manifest.configPath) !== win32.resolve(win32.join(process.env.APPDATA, APP_IDENTIFIER, 'config.json'))) {
      fail('run journal does not match requested config');
    }
    const lockPath = win32.join(dirname(manifest.configPath), 'alpha-harness.lock.json');
    if (!existsSync(lockPath)) {
      const current = readFileSync(manifest.configPath);
      const original = readFileSync(manifest.originalPath);
      if (!current.equals(original)) fail('config lock is gone but original config is not restored');
      const windowState = checkedWindowStateEvidence(manifest);
      const profile = restoreProfileFolders(manifest.profileRedirects);
      return { restored: true, alreadyRestored: true, ...windowState, profile, ...removeRunArtifacts(tools, appPath, manifest, paths) };
    }
    const lock = JSON.parse(readFileSync(lockPath, 'utf8'));
    if (lock.runId !== request.runId) fail('config lock belongs to another run');
    const windowState = restoreJournaledFiles(manifest);
    // A run that failed between break-config and repair-config leaves the app's damaged copy.
    // Moved while the lock still stands, so a failure here leaves the run for a retry to finish.
    const damagedMoved = moveRunDamagedConfigs(dirname(manifest.configPath), paths);
    const profile = restoreProfileFolders(manifest.profileRedirects);
    unlinkSync(lockPath);
    return { restored: true, damagedMoved, ...windowState, profile, ...removeRunArtifacts(tools, appPath, manifest, paths) };
  }

  fail(`unknown Windows action: ${request.action}`);
}

// ── #28 acceptance gate ──
//
// Both installed apps run at once under WebDriver and every product action goes through their
// UI. Syncthing's REST API, the vault trees, and the backup archives are only read, to check what
// the UI did. The one write outside the UI is the receiver's ignore list for the delayed
// attachment, which stands in for bytes that are slow to arrive. Each #28 criterion ends in one
// `criterion` line naming the evidence it rests on.

// Every file in the vault with its SHA-256, the files whose text holds each marker, and the
// Syncthing conflict copies among them.
export function vaultSummary(vault, markers = []) {
  const files = {};
  const found = Object.fromEntries(markers.map((marker) => [marker, []]));
  const visit = (directory, prefix) => {
    for (const entry of readdirSync(directory, { withFileTypes: true }).sort(byName)) {
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
      const path = join(directory, entry.name);
      if (entry.isDirectory()) visit(path, relative);
      else if (entry.isFile()) {
        const data = readFileSync(path);
        files[relative] = createHash('sha256').update(data).digest('hex');
        const text = data.toString('utf8');
        for (const marker of markers) if (text.includes(marker)) found[marker].push(relative);
      }
    }
  };
  visit(vault, '');
  // A copy archived under .helixnotes/trash after a choice is no longer a conflict.
  return { files, markers: found, conflicts: Object.keys(files).filter((path) => path.includes('.sync-conflict-') && !path.startsWith('.helixnotes/')) };
}

// What one backup archive holds for the paths asked about (SHA-256, or null when absent), and
// which members hold each marker.
export function backupSummary(archive, wanted = [], markers = []) {
  const entries = zipEntries(readFileSync(archive));
  const hashOf = (path) => (entries.has(path) ? createHash('sha256').update(entries.get(path)).digest('hex') : null);
  const found = Object.fromEntries(markers.map((marker) => [marker, [...entries]
    .filter(([name, data]) => !name.endsWith('/') && data.toString('utf8').includes(marker)).map(([name]) => name)]));
  return { members: entries.size, wanted: Object.fromEntries(wanted.map((path) => [path, hashOf(path)])), markers: found };
}

// A solid-colour PNG, so every image in the run has its own bytes.
export function solidPng(width, height, [red, green, blue]) {
  const chunk = (kind, data) => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(kind, 'ascii'), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, crc]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const row = Buffer.concat([Buffer.from([0]), Buffer.alloc(width * 3).fill(Buffer.from([red, green, blue]))]);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(Buffer.concat(Array.from({ length: height }, () => row)))),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// Fedora's whole connection loses its default routes, so nothing on the laptop reaches the
// internet; the on-link LAN route stays, and with it Tailscale's direct path to the desktop. A
// user timer armed first puts the routes back at the deadline even if this process dies.
const ISOLATION_MINUTES = 20;
function fedoraNetwork(runId) {
  const [route] = JSON.parse(runCommandSync('ip', ['-j', 'route', 'show', 'default']).stdout || '[]');
  if (!route?.dev) fail('Fedora has no default route to withdraw');
  const device = route.dev;
  const connection = runCommandSync('nmcli', ['-g', 'GENERAL.CONNECTION', 'device', 'show', device]).stdout;
  if (!connection) fail(`no NetworkManager connection on ${device}`);
  const unit = `second-brain-acceptance-net-${runId.toLowerCase()}`;
  const setDefault = (withdrawn) => {
    const value = withdrawn ? 'yes' : 'no';
    runCommandSync('nmcli', ['connection', 'modify', connection, 'ipv4.never-default', value, 'ipv6.never-default', value]);
    runCommandSync('nmcli', ['device', 'reapply', device]);
  };
  const exitStatus = (executable, args) => runCommandSync(executable, args, { accept: [0, 1, 2, 6, 7, 28, 35], timeout: 20_000 });
  let isolated = false;
  return {
    probe(peerIp) {
      const route4 = exitStatus('ip', ['route', 'get', '1.1.1.1']);
      const route6 = exitStatus('ip', ['-6', 'route', 'get', '2606:4700:4700::1111']);
      const https = (url) => exitStatus('curl', ['-s', '-o', '/dev/null', '-w', '%{http_code}', '--max-time', '8', url]);
      const byAddress = https('https://1.1.1.1');
      const byName = https('https://www.wikipedia.org');
      const ping = exitStatus('tailscale', ['ping', '-c', '1', peerIp]);
      return {
        publicRoute4: route4.status === 0,
        publicRoute6: route6.status === 0,
        httpsByAddress: byAddress.status === 0 ? Number(byAddress.stdout) : `curl exit ${byAddress.status}`,
        httpsByName: byName.status === 0 ? Number(byName.stdout) : `curl exit ${byName.status}`,
        // "via <LAN address>" is a direct path; "via DERP" would need the internet.
        tailscalePath: /via (DERP\([^)]*\)|[\d.]+:\d+)/.exec(ping.stdout)?.[1] ?? `ping exit ${ping.status}`,
      };
    },
    isolate() {
      const original = runCommandSync('nmcli', ['-g', 'ipv4.never-default,ipv6.never-default', 'connection', 'show', connection]).stdout.split('\n');
      if (original.some((value) => value !== 'no')) fail(`${connection} already withholds its default route: ${original}`);
      runCommandSync('systemd-run', [
        '--user', `--unit=${unit}`, `--on-active=${ISOLATION_MINUTES * 60}`, '/usr/bin/sh', '-c',
        'nmcli connection modify "$1" ipv4.never-default no ipv6.never-default no && nmcli device reapply "$2"',
        'sh', connection, device,
      ]);
      isolated = true;
      setDefault(true);
      return { device, rollback: `${unit}.timer in ${ISOLATION_MINUTES} min` };
    },
    async restore() {
      if (!isolated) return { restored: false };
      setDefault(false);
      runCommandSync('systemctl', ['--user', 'stop', `${unit}.timer`], { accept: [0, 5] });
      isolated = false;
      await waitForState(() => exitStatus('ip', ['route', 'get', '1.1.1.1']).status, (status) => status === 0, 60_000, 'Fedora default route');
      return { restored: true };
    },
  };
}

const ACCEPTANCE_CLIP_BEFORE = 'https://en.wikipedia.org/wiki/Zettelkasten';
// A host no earlier step reached, so no pooled connection can carry the isolated request.
const ACCEPTANCE_CLIP_DURING = 'https://www.gutenberg.org/';
const ACCEPTANCE_CLIP_AFTER = 'https://en.wikipedia.org/wiki/Commonplace_book';

async function runAcceptance(args) {
  const options = parseOptions(args);
  if (!options.fedoraRpm) fail('--fedora-rpm names the installed candidate RPM, which verify-linux-package.sh checks');
  return runOnFedora(args, runAcceptanceLocked);
}

async function runAcceptanceLocked(options) {
  const { candidateCommit, runId, evidencePath, screenshotDir } = openGateTrace(options, 'acceptance', {
    runStart: { issue: 28 },
    packageChecks: () => fedoraPackageChecks(options.fedoraRpm),
  });
  const record = (event, machine, value) => observation(evidencePath, event, machine, value);
  const vaultId = randomUUID();
  // No Ollama: the gate needs keyword search only, and nothing may reach out on its behalf.
  const network = fedoraNetwork(runId);
  const sides = [];
  let fedora;
  let windows;
  let windowsPrepared = false;
  let runError;
  let cleanupError;
  try {
    fedora = linuxDriverMachine(options.linuxRoot, runId, vaultId, { ollamaBaseUrl: OFFLINE_OLLAMA_URL });
    windows = windowsDriverMachine(options.sshHost, runId, vaultId, candidateCommit, { ollamaBaseUrl: OFFLINE_OLLAMA_URL });
    record('prepared', 'fedora', fedora.prepare());
    windowsPrepared = true;
    record('prepared', 'windows', windows.prepare());
    for (const [machine, realKeys] of [[fedora, false], [windows, true]]) {
      record('driver-started', machine.name, await machine.startDriver());
      sides.push({ name: machine.name, machine, type: (await import('./alpha-walkthrough.mjs')).typist(realKeys), browser: await openApp(machine.port, machine.application) });
    }
    await acceptanceCriteria({ sides, runId, vaultId, record, screenshotDir, network, sshHost: options.sshHost });
  } catch (error) {
    runError = error;
    record('run-failed', 'controller', { error: error.message });
  } finally {
    // Connectivity comes back first, whatever else failed.
    try {
      record('network-final', 'fedora', await network.restore());
    } catch (error) {
      record('cleanup-error', 'fedora-network', { error: error.message });
      cleanupError ??= error;
    }
    if (windowsPrepared) {
      try {
        record('isolation-final', 'windows', windows.isolation('restore'));
      } catch (error) {
        record('cleanup-error', 'windows-isolation', { error: error.message });
        cleanupError ??= error;
      }
    }
    for (const side of sides) await closeApp(side.browser);
    if (fedora) cleanupError ??= await finishMachine(fedora, evidencePath);
    if (windowsPrepared) cleanupError ??= await finishMachine(windows, evidencePath);
  }
  if (runError) throw runError;
  if (cleanupError) throw cleanupError;
  record('run-complete', 'controller', { criteria: 11 });
  console.log(JSON.stringify({ runId, evidencePath, screenshotDir }));
}

async function acceptanceCriteria({ sides, runId, vaultId, record, screenshotDir, network, sshHost }) {
  const walkthrough = await import('./alpha-walkthrough.mjs');
  const [F, W] = sides;
  const peerOf = (side) => (side === F ? W : F);
  const tag = runId.toLowerCase();
  // Keyword search splits on non-word characters, so markers are single words.
  const marker = (name) => `zq${name}${tag}`.replace(/[^a-z0-9]/g, '');
  const shot = (side, label) => side.browser.saveScreenshot(join(screenshotDir, `${side.name}-${label}.png`)).catch(() => {});
  const step = async (label, action) => {
    const started = Date.now();
    try {
      const value = await action();
      record('step', 'controller', { step: label, ms: Date.now() - started, ...(value && typeof value === 'object' ? { value } : {}) });
      return value;
    } catch (error) {
      await Promise.all(sides.map((side) => shot(side, `${label}-failed`)));
      throw new Error(`${label}: ${error.message}`);
    }
  };
  const criterion = (number, text, evidence) => record('criterion', 'controller', { number, text, passed: true, ...evidence });
  const closeSettings = (side) => walkthrough.closeSettingsPanel(side.browser);
  // Diagnostics only: the page's errors and when its editor first showed Unsaved, so a stuck
  // close names its cause.
  const instrument = (side) => side.browser.execute(() => {
    if (window.__acceptance) return;
    window.__acceptance = { errors: [], dirtySince: null };
    const original = console.error;
    console.error = (...parts) => {
      window.__acceptance.errors.push(parts.map((part) => part?.message ?? String(part)).join(' ').slice(0, 500));
      original(...parts);
    };
    window.addEventListener('unhandledrejection', (event) => window.__acceptance.errors.push(`unhandled: ${String(event.reason?.message ?? event.reason).slice(0, 500)}`));
    setInterval(() => {
      const dirty = Boolean(document.querySelector('.save-indicator'));
      if (dirty && !window.__acceptance.dirtySince) window.__acceptance.dirtySince = { at: new Date().toISOString(), title: document.querySelector('.editor-title input')?.value ?? null };
      if (!dirty) window.__acceptance.dirtySince = null;
    }, 250);
  });
  const editorState = (side) => side.browser.execute(() => ({
    title: document.querySelector('.editor-title input')?.value ?? null,
    dirty: Boolean(document.querySelector('.save-indicator')),
    viewMode: Boolean(document.querySelector('.readonly-indicator')),
    ...(window.__acceptance ?? {}),
  })).catch((error) => ({ error: error.message }));
  for (const side of sides) await instrument(side);
  const relaunch = async (side) => {
    const before = await editorState(side);
    record('before-close', side.name, before);
    await walkthrough.closeWindow(side.browser);
    try {
      await side.machine.appsGone();
    } catch (error) {
      record('close-stuck', side.name, await editorState(side));
      await shot(side, 'close-stuck');
      throw error;
    }
    await closeApp(side.browser);
    side.browser = await openApp(side.machine.port, side.machine.application);
    await instrument(side);
  };

  // A note file is saved by autosave; the check waits for its bytes on the machine itself.
  const waitForFile = (side, predicate, what, timeoutMs = 60_000) => waitForState(
    () => side.machine.vaultSummary(predicate.markers ?? []), (summary) => predicate(summary), timeoutMs, `${side.name}: ${what}`,
  );
  const preSync = (side) => side.machine.backups().filter((name) => name.startsWith('helixnotes-pre-sync-')).sort();

  // One guarded batch on both machines: Sync now on each, as the Settings hint says, and both
  // terminal outcomes. A scheduled run may own the lease when Sync now is pressed; `until` is the
  // state the batch has to reach, so a batch is repeated (at most three times) until it holds.
  // The receiver's first new pre-sync backup must hold `prior`, the receiver's own state of the
  // watched paths from before the change was made (criterion 9).
  const syncBatch = async (label, { mark, receiver, prior, until }) => {
    const attempts = [];
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      await Promise.all(sides.map((side) => walkthrough.pressSyncNow(side.browser)));
      const outcomes = await Promise.all(sides.map((side) => walkthrough.syncDone(side.browser)));
      const reached = await until();
      attempts.push(Object.fromEntries(sides.map((side, index) => [side.name, outcomes[index]])));
      if (reached) break;
      if (attempt === 3) fail(`${label}: not reached after three batches: ${JSON.stringify(attempts)}`);
    }
    for (const side of sides) await closeSettings(side);
    const fresh = Object.fromEntries(sides.map((side) => [side.name, preSync(side).filter((name) => !mark[side.name].includes(name))]));
    let backup = null;
    if (receiver) {
      const [first] = fresh[receiver.name];
      if (!first) fail(`${label}: ${receiver.name} received changes without a new pre-sync backup`);
      backup = { name: first, ...receiver.machine.backupSummary(first, Object.keys(prior), []) };
      const mismatched = Object.entries(prior).filter(([path, hash]) => backup.wanted[path] !== hash);
      if (mismatched.length) fail(`${label}: backup ${first} does not hold ${receiver.name}'s previous state: ${JSON.stringify({ mismatched, backup: backup.wanted })}`);
    }
    const value = { label, attempts, newBackups: fresh, backup, editors: Object.fromEntries(await Promise.all(sides.map(async (side) => [side.name, await editorState(side)]))) };
    record('sync-batch', 'controller', value);
    return value;
  };
  const markBackups = () => Object.fromEntries(sides.map((side) => [side.name, preSync(side)]));
  const priorOf = (side, paths) => {
    const { files } = side.machine.vaultSummary();
    return Object.fromEntries(paths.map((path) => [path, files[path] ?? null]));
  };

  // Notion stays unconfigured on both machines for the whole gate (criterion 10).
  for (const side of sides) {
    if (side.machine.notionTokenStored()) fail(`${side.name} keyring already holds this vault's Notion token`);
  }

  // ── 1. Sync is a toggle in Settings; nothing to install ──
  const bundled = {};
  for (const side of sides) {
    bundled[side.name] = await step(`${side.name}-toggle`, async () => {
      await walkthrough.vaultOpened(side.browser);
      const before = side.machine.syncRoles().map((row) => row.role);
      if (before.includes('Sidecar')) fail(`sidecar runs before sync was enabled: ${before}`);
      const roleOf = () => side.machine.syncRoles();
      await walkthrough.setSync(side.browser, true);
      const on = await waitForState(roleOf, (rows) => syncProcessesUp(rows.map((row) => row.role)), 60_000, 'sidecar start');
      const sidecar = on.find((row) => row.role === 'Sidecar');
      const expected = side === F ? '/usr/bin/syncthing' : win32.join(win32.dirname(WINDOWS_APP), 'syncthing.exe');
      if (sidecar.executable.toLowerCase() !== expected.toLowerCase()) fail(`sidecar is not the bundled executable: ${sidecar.executable}`);
      const owner = side === F ? runCommandSync('rpm', ['-qf', '--qf', '%{NAME}', expected]).stdout : 'installer';
      if (side === F && owner !== 'second-brain') fail(`${expected} belongs to ${owner}`);
      await walkthrough.setSync(side.browser, false);
      await waitForState(roleOf, (rows) => !rows.some((row) => row.role === 'Sidecar'), 60_000, 'sidecar stop');
      await walkthrough.setSync(side.browser, true);
      await waitForState(roleOf, (rows) => syncProcessesUp(rows.map((row) => row.role)), 60_000, 'sidecar restart');
      await shot(side, 'c1-sync-enabled');
      return { sidecar: expected, package: owner, before, toggled: ['on', 'off', 'on'] };
    });
  }
  criterion(1, 'Sync is a toggle in Settings; the user installs nothing and configures no second application', { bundled });

  // ── 2. Explicit one-time pairing, not Tailnet membership ──
  const identities = {};
  for (const side of sides) {
    const shown = await walkthrough.syncIdentity(side.browser);
    if (!DEVICE_ID.test(shown.deviceId ?? '')) fail(`${side.name} Settings shows no device ID`);
    if (shown.vaultId !== vaultId) fail(`${side.name} Settings shows vault ${shown.vaultId}`);
    const status = await side.machine.syncthing('GET', '/rest/system/status');
    if (status.myID !== shown.deviceId) fail(`${side.name} Settings device ID differs from its sidecar`);
    identities[side.name] = { ...shown, tailscaleIp: side === F ? localTailscaleIp() : null };
  }
  const windowsIdentity = remoteWorker(sshHost, { action: 'identity', root: WINDOWS_ROOT, runId });
  if (windowsIdentity.deviceId !== identities.windows.deviceId) fail('Windows sidecar reports another device ID');
  identities.windows.tailscaleIp = windowsIdentity.tailscaleIp;
  const peerShape = async (side) => {
    const config = await side.machine.syncthing('GET', '/rest/config');
    const peerId = identities[peerOf(side).name].deviceId;
    return {
      devices: config.devices.map((device) => (device.deviceID === peerId ? 'peer' : device.deviceID === identities[side.name].deviceId ? 'self' : 'other')),
      folders: config.folders.map((folder) => ({ id: folder.id === vaultId ? 'vault' : 'other', type: folder.type, paused: folder.paused, sharedWithPeer: folder.devices.some((device) => device.deviceID === peerId) })),
      autoAccept: config.devices.filter((device) => device.autoAcceptFolders).length,
    };
  };
  const pairing = await step('pairing', async () => {
    const unpaired = Object.fromEntries(await Promise.all(sides.map(async (side) => [side.name, await peerShape(side)])));
    for (const [name, shape] of Object.entries(unpaired)) {
      if (shape.devices.includes('peer') || shape.folders.some((folder) => folder.sharedWithPeer)) {
        fail(`${name} knows its peer before pairing, though both sidecars are on the Tailnet: ${JSON.stringify(shape)}`);
      }
    }
    const details = (side) => ({ name: side === F ? 'Windows desktop' : 'Fedora laptop', deviceId: identities[side.name].deviceId, tailscaleIp: identities[side.name].tailscaleIp, vaultId });
    const wrong = await walkthrough.pairDevice(F.browser, F.type, { ...details(W), vaultId: randomUUID() });
    if (wrong.status.paired || /^Paired with/.test(wrong.message)) fail(`a mismatched vault ID paired: ${wrong.message}`);
    const afterWrong = await peerShape(F);
    if (JSON.stringify(afterWrong) !== JSON.stringify(unpaired.fedora)) fail(`a refused pairing changed Fedora's config: ${JSON.stringify(afterWrong)}`);
    const paired = {};
    for (const side of sides) {
      const result = await walkthrough.pairDevice(side.browser, side.type, details(peerOf(side)));
      if (!result.status.paired || !/^Paired with/.test(result.message)) fail(`${side.name} did not pair: ${result.message}`);
      paired[side.name] = { message: result.message, shape: await peerShape(side) };
      if (!paired[side.name].shape.folders.some((folder) => folder.id === 'vault' && folder.sharedWithPeer && folder.type === 'sendonly')) {
        fail(`${side.name} vault folder is not shared send-only with its peer: ${JSON.stringify(paired[side.name].shape)}`);
      }
      await shot(side, 'c2-paired');
    }
    return { unpaired, refused: { message: wrong.message, configUnchanged: true }, paired };
  });

  // ── 3. A note created on one machine is found by search on the other ──
  const roundTrip = async (from, to, letter) => {
    const word = marker(`note${letter}`);
    const note = { category: 'Projects', title: `Acceptance ${letter} ${runId}`, body: `Received on the other machine: ${word}.` };
    const relativePath = `${note.category}/${note.title}.md`;
    const mark = markBackups();
    const prior = priorOf(to, [relativePath]);
    await walkthrough.createNote(from.browser, from.type, note);
    const sent = await waitForFile(from, Object.assign((summary) => summary.markers[word].includes(relativePath), { markers: [word] }), `note ${letter} saved`);
    const batch = await syncBatch(`note-${letter}`, {
      mark, receiver: to, prior,
      until: () => to.machine.vaultSummary([word]).markers[word].includes(relativePath),
    });
    const titles = await walkthrough.keywordTitles(to.browser, to.type, word);
    if (!titles.includes(note.title)) fail(`${to.name} keyword search for ${word} did not find "${note.title}": ${JSON.stringify(titles)}`);
    const opened = await walkthrough.openNoteIn(to.browser, note.category, note.title);
    if (!opened.text.includes(word)) fail(`${to.name} opened "${note.title}" without its text`);
    await shot(to, `c3-found-${letter}`);
    return { relativePath, from: from.name, to: to.name, searchTitles: titles, backup: batch.backup.name };
  };
  const noteA = await step('note-fedora-to-windows', () => roundTrip(F, W, 'a'));
  const noteB = await step('note-windows-to-fedora', () => roundTrip(W, F, 'b'));
  criterion(2, 'Two machines pair through an explicit one-time exchange, not by being on the same Tailnet', { pairing: { refused: pairing.refused.message, paired: Object.keys(pairing.paired) }, oneTime: 'checked again after relaunch in criterion 5' });
  criterion(3, 'A note created on one machine appears on the other, and is findable by search there', { notes: [noteA, noteB] });

  // ── 4. A received attachment opens normally ──
  const attachmentPath = (summary, name) => Object.keys(summary.files).find((path) => path.startsWith('.helixnotes/attachments/') && path.endsWith(`_${name}`));
  const sendAttachments = async (from, to, letter, { image, text }) => {
    const word = marker(`att${letter}`);
    const note = { category: 'Resources', title: `Attachment ${letter} ${runId}`, body: `Attachments for ${word}. ` };
    const notePath = `${note.category}/${note.title}.md`;
    const mark = markBackups();
    const attachmentNames = [image?.name, text?.name].filter(Boolean);
    const prior = priorOf(to, [notePath]);
    await walkthrough.createNote(from.browser, from.type, note);
    if (image) {
      const bytes = image.bytes.toString('base64');
      await walkthrough.attachImage(from.browser, { name: image.name, base64: bytes, path: from.machine.stageBinary(image.name, bytes) });
    }
    if (text) await walkthrough.attachFile(from.browser, { name: text.name, content: text.content, path: from.machine.stageFile(text.name, text.content) });
    const sent = await waitForFile(from, (summary) => attachmentNames.every((name) => {
      const path = attachmentPath(summary, name);
      return path && summary.files[notePath] && readFileText(from, notePath).includes(path.split('/').pop());
    }), `note ${letter} saved with its attachments`);
    const paths = Object.fromEntries(attachmentNames.map((name) => [name, attachmentPath(sent, name)]));
    return { note, notePath, word, mark, prior, sent, paths };
  };
  const readFileText = (side, relativePath) => side.machine.readNote(relativePath);
  const expectRendered = async (side, sentPath, label) => {
    let images = [];
    await side.browser.waitUntil(async () => (images = await walkthrough.editorImages(side.browser))
      .some((image) => image.naturalWidth > 0 && !image.pending && decodeURIComponent(image.src ?? '').includes(sentPath.split('/').pop())), {
      timeout: 60_000, timeoutMsg: `${side.name}: ${label} did not render: ${JSON.stringify(images)}`,
    }).catch((error) => fail(error.message));
    return images.find((image) => image.naturalWidth > 0);
  };
  const attachment = await step('attachment-fedora-to-windows', async () => {
    const image = { name: `photo-${tag}.png`, bytes: solidPng(64, 48, [200, 40, 90]) };
    const text = { name: `notes-${tag}.txt`, content: `Attachment text ${marker('txt')}\n` };
    const sent = await sendAttachments(F, W, 'c', { image, text });
    const batch = await syncBatch('attachment', {
      mark: sent.mark, receiver: W, prior: sent.prior,
      until: () => {
        const { files } = W.machine.vaultSummary();
        return Object.values(sent.paths).every((path) => files[path] === sent.sent.files[path]) && Boolean(files[sent.notePath]);
      },
    });
    const received = W.machine.vaultSummary().files;
    await walkthrough.openNoteIn(W.browser, sent.note.category, sent.note.title);
    const rendered = await expectRendered(W, sent.paths[image.name], 'received image');
    if (rendered.naturalWidth !== 64) fail(`received image is ${rendered.naturalWidth}px wide, not 64`);
    await shot(W, 'c4-attachment-opened');
    return {
      image: { path: sent.paths[image.name], sha256: received[sent.paths[image.name]], sameBytes: received[sent.paths[image.name]] === sent.sent.files[sent.paths[image.name]], naturalWidth: rendered.naturalWidth },
      // Bytes only: opening a non-image hands it to the operating system, which this gate does not drive.
      textFile: { path: sent.paths[text.name], sameBytes: received[sent.paths[text.name]] === sent.sent.files[sent.paths[text.name]], proves: 'transfer only' },
      backup: batch.backup.name,
    };
  });
  criterion(4, 'An attachment created on one machine arrives on the other and opens normally', { attachment });

  // ── 7. A delayed attachment shows as not synced yet, then opens ──
  const delayed = await step('delayed-attachment', async () => {
    const image = { name: `delayed-${tag}.png`, bytes: solidPng(40, 30, [20, 160, 70]) };
    const pattern = `/.helixnotes/attachments/*_${image.name}`;
    const ignoresPath = `/rest/db/ignores?folder=${encodeURIComponent(vaultId)}`;
    const original = (await W.machine.syncthing('GET', ignoresPath)).ignore ?? [];
    await W.machine.syncthing('POST', ignoresPath, { ignore: [...original, pattern] });
    const withPattern = (await W.machine.syncthing('GET', ignoresPath)).ignore ?? [];
    if (!withPattern.includes(pattern)) fail(`receiver ignore list did not take ${pattern}`);
    let restored = false;
    const restoreIgnores = async () => {
      if (restored) return;
      await W.machine.syncthing('POST', ignoresPath, { ignore: original });
      restored = true;
    };
    try {
      const sent = await sendAttachments(F, W, 'd', { image });
      const imagePath = sent.paths[image.name];
      await syncBatch('delayed-note', {
        mark: sent.mark, receiver: W, prior: sent.prior,
        until: () => Boolean(W.machine.vaultSummary().files[sent.notePath]),
      });
      if (W.machine.vaultSummary().files[imagePath]) fail('the withheld attachment arrived anyway');
      await walkthrough.openNoteIn(W.browser, sent.note.category, sent.note.title);
      let pending = [];
      await W.browser.waitUntil(async () => (pending = await walkthrough.editorImages(W.browser)).some((img) => img.pending), {
        timeout: 30_000, timeoutMsg: 'the missing attachment was not marked as not synced yet',
      }).catch(() => fail(`no pending image: ${JSON.stringify(pending)}`));
      const placeholder = pending.find((img) => img.pending);
      if (placeholder.alt !== 'Attachment not synced yet') fail(`placeholder reads "${placeholder.alt}"`);
      // The same element must load later, with the note still open: no reopen, no reload.
      await W.browser.execute(() => { document.querySelector('.ProseMirror img[data-sync-pending="true"]').dataset.harness = 'delayed'; });
      await shot(W, 'c7-not-synced-yet');
      await restoreIgnores();
      const mark = markBackups();
      await syncBatch('delayed-bytes', {
        mark, receiver: W, prior: { [imagePath]: null },
        until: () => W.machine.vaultSummary().files[imagePath] === sent.sent.files[imagePath],
      });
      let loaded = null;
      await W.browser.waitUntil(async () => (loaded = await W.browser.execute(() => {
        const img = document.querySelector('.ProseMirror img[data-harness="delayed"]');
        return img ? { naturalWidth: img.naturalWidth, pending: img.dataset.syncPending === 'true', alt: img.alt } : null;
      }))?.naturalWidth > 0 && !loaded.pending, { timeout: 60_000, timeoutMsg: 'the delayed image did not load in place' }).catch(() => fail(`delayed image did not load in place: ${JSON.stringify(loaded)}`));
      await shot(W, 'c7-arrived');
      return { pattern, placeholder: placeholder.alt, loadedInPlace: loaded, sameBytes: true };
    } finally {
      await restoreIgnores();
    }
  });
  criterion(7, 'An attachment that has not yet arrived displays as not-synced-yet, never as broken or missing', { delayed });

  // Edits made while the machines cannot sync: sync is turned off on both through Settings and
  // their sidecars must be gone, so no scheduled batch can run in between.
  const disconnected = async (label, edits) => {
    for (const side of sides) {
      await walkthrough.setSync(side.browser, false);
      await waitForState(() => side.machine.syncRoles(), (rows) => !rows.some((row) => row.role === 'Sidecar'), 60_000, `${side.name} sidecar stop`);
      await closeSettings(side);
    }
    const value = await edits();
    for (const side of sides) {
      await walkthrough.setSync(side.browser, true);
      await waitForState(() => side.machine.syncRoles(), (rows) => syncProcessesUp(rows.map((row) => row.role)), 60_000, `${side.name} sidecar restart`);
      await closeSettings(side);
    }
    record('disconnected-edits', 'controller', { label });
    return value;
  };

  // ── 5 and 6. Delete and PARA move while the peer is away ──
  const deleteMove = await step('delete-and-move', async () => {
    const doomed = { category: 'Areas', title: `Delete me ${runId}`, body: `Deleted on Fedora: ${marker('del')}.` };
    const moved = { category: 'Projects', title: `Move me ${runId}`, body: `Moved on Fedora: ${marker('mov')}.` };
    const doomedPath = `${doomed.category}/${doomed.title}.md`;
    const fromPath = `${moved.category}/${moved.title}.md`;
    const toPath = `Archives/${moved.title}.md`;
    const mark0 = markBackups();
    const prior0 = priorOf(W, [doomedPath, fromPath]);
    for (const note of [doomed, moved]) await walkthrough.createNote(F.browser, F.type, note);
    const words = [marker('del'), marker('mov')];
    const sent = await waitForFile(F, Object.assign((summary) => summary.markers[words[0]].includes(doomedPath) && summary.markers[words[1]].includes(fromPath), { markers: words }), 'delete and move notes saved');
    await syncBatch('delete-move-setup', {
      mark: mark0, receiver: W, prior: prior0,
      until: () => {
        const { files } = W.machine.vaultSummary();
        return Boolean(files[doomedPath]) && Boolean(files[fromPath]);
      },
    });
    const mark = markBackups();
    const prior = priorOf(W, [doomedPath, fromPath, toPath]);
    if (!prior[doomedPath] || !prior[fromPath]) fail('Windows does not hold both notes before the delete and move');
    await disconnected('delete-move', async () => {
      await walkthrough.moveNote(F.browser, { from: moved.category, to: 'Archives', title: moved.title });
      await deleteThroughMenu(F, doomed);
    });
    const fedoraAfter = F.machine.vaultSummary(words);
    if (fedoraAfter.files[doomedPath] || !fedoraAfter.files[toPath] || fedoraAfter.files[fromPath]) fail(`Fedora did not delete and move: ${JSON.stringify(Object.keys(fedoraAfter.files).filter((path) => path.endsWith('.md')))}`);
    await syncBatch('delete-move', {
      mark, receiver: W, prior,
      until: () => {
        const { files } = W.machine.vaultSummary();
        return !files[doomedPath] && !files[fromPath] && Boolean(files[toPath]);
      },
    });
    const check = async (label) => {
      const result = {};
      for (const side of sides) {
        const summary = side.machine.vaultSummary(words);
        const outsideTrash = (word) => summary.markers[word].filter((path) => !path.startsWith('.helixnotes/'));
        if (outsideTrash(words[0]).length) fail(`${label}: deleted note is back on ${side.name}: ${outsideTrash(words[0])}`);
        if (JSON.stringify(outsideTrash(words[1])) !== JSON.stringify([toPath])) fail(`${label}: moved note on ${side.name} is at ${JSON.stringify(outsideTrash(words[1]))}`);
        if (!summary.markers[words[0]].some((path) => path.startsWith('.helixnotes/trash/'))) fail(`${label}: deleted note is not in ${side.name}'s trash`);
        const deletedHits = await walkthrough.keywordTitles(side.browser, side.type, words[0]);
        const movedHits = await walkthrough.keywordTitles(side.browser, side.type, words[1]);
        if (deletedHits.includes(doomed.title)) fail(`${label}: ${side.name} search still finds the deleted note`);
        if (movedHits.filter((title) => title === moved.title).length !== 1) fail(`${label}: ${side.name} search finds the moved note ${movedHits.length} times`);
        const archived = await walkthrough.categoryTitles(side.browser, 'Archives');
        const projects = await walkthrough.categoryTitles(side.browser, 'Projects');
        if (archived.filter((title) => title === moved.title).length !== 1 || projects.includes(moved.title)) fail(`${label}: ${side.name} lists the moved note wrongly`);
        result[side.name] = { deletedSearchHits: deletedHits.length, movedSearchHits: movedHits.length, movedAt: toPath, trashed: true };
      }
      return result;
    };
    const afterSync = await check('after sync');
    // No resurrection, and no second pairing: both apps start again and sync from what they have.
    for (const side of sides) await relaunch(side);
    const mark2 = markBackups();
    await syncBatch('after-relaunch', { mark: mark2, until: async () => true });
    const afterRelaunch = await check('after relaunch');
    for (const side of sides) {
      const status = await walkthrough.syncStatus(side.browser);
      if (!status.paired) fail(`${side.name} is no longer paired after relaunch`);
    }
    await Promise.all(sides.map((side) => shot(side, 'c5-c6-after-relaunch')));
    return { doomedPath, moved: { from: fromPath, to: toPath }, afterSync, afterRelaunch, pairedAfterRelaunch: true };
  });
  criterion(5, 'A deletion propagates; a note deleted on one machine does not resurrect from the other', { deleted: deleteMove.doomedPath, afterSync: deleteMove.afterSync, afterRelaunch: deleteMove.afterRelaunch });
  criterion(6, 'Moving a note between PARA categories on one machine does not duplicate or lose it on the other', { moved: deleteMove.moved });
  criterion(2, 'Pairing stays after relaunch; no second exchange', { pairedAfterRelaunch: deleteMove.pairedAfterRelaunch, part: 'one-time' });

  // ── 8. Conflicts surface both versions; the copy is never an ordinary note ──
  const conflict = await step('conflict', async () => {
    const notes = ['keep', 'use'].map((name) => ({ name, category: 'Areas', title: `Conflict ${name} ${runId}`, body: `Base text ${marker(`base${name}`)}.` }));
    const pathOf = (note) => `${note.category}/${note.title}.md`;
    const mark0 = markBackups();
    const prior0 = priorOf(W, notes.map(pathOf));
    for (const note of notes) await walkthrough.createNote(F.browser, F.type, note);
    const baseWords = notes.map((note) => marker(`base${note.name}`));
    const sent = await waitForFile(F, Object.assign((summary) => notes.every((note, index) => summary.markers[baseWords[index]].includes(pathOf(note))), { markers: baseWords }), 'conflict notes saved');
    await syncBatch('conflict-setup', {
      mark: mark0, receiver: W, prior: prior0,
      until: () => { const { files } = W.machine.vaultSummary(); return notes.every((note) => Boolean(files[pathOf(note)])); },
    });
    const editWord = (side, note) => marker(`${side.name.slice(0, 3)}${note.name}`);
    const mark = markBackups();
    await disconnected('conflict', async () => {
      for (const side of sides) {
        for (const note of notes) {
          await walkthrough.editNote(side.browser, side.type, { category: note.category, title: note.title, text: ` Edited on ${side.name}: ${editWord(side, note)}.` });
        }
      }
      for (const side of sides) {
        const words = notes.map((note) => editWord(side, note));
        await waitForFile(side, Object.assign((summary) => notes.every((note, index) => summary.markers[words[index]].includes(pathOf(note))), { markers: words }), 'conflicting edits saved');
      }
    });
    const allWords = sides.flatMap((side) => notes.map((note) => editWord(side, note)));
    await syncBatch('conflict', {
      mark,
      until: () => sides.every((side) => side.machine.vaultSummary().conflicts.length === notes.length),
    });
    // Which edit lost is Syncthing's choice; the conflict-only words are read from the copies.
    const listed = {};
    for (const side of sides) {
      const { conflicts, shown } = await walkthrough.listConflicts(side.browser);
      if (conflicts.length !== notes.length || shown.length !== notes.length) fail(`${side.name} Settings lists ${shown.length} conflicts`);
      listed[side.name] = conflicts;
    }
    const resolver = W;
    const plan = notes.map((note) => {
      // relativePath names the conflict copy: `<category>/<title>.sync-conflict-<stamp>-<device>.md`.
      const entry = listed[resolver.name].find((item) => item.relativePath.startsWith(`${note.category}/${note.title}.sync-conflict-`));
      if (!entry) fail(`no conflict for ${pathOf(note)}`);
      const copyWord = allWords.find((word) => entry.conflictContent.includes(word) && !(entry.originalContent ?? '').includes(word));
      const currentWord = allWords.find((word) => (entry.originalContent ?? '').includes(word) && !entry.conflictContent.includes(word));
      if (!copyWord || !currentWord) fail(`conflict on ${pathOf(note)} does not hold the two edits`);
      return { note, entry, copyWord, currentWord, choice: note.name === 'keep' ? 'Keep current' : 'Use conflict' };
    });
    const exclusion = {};
    for (const side of sides) {
      const counts = await walkthrough.paraCounts(side.browser);
      const summary = side.machine.vaultSummary();
      const ordinary = (category) => Object.keys(summary.files).filter((path) => path.startsWith(`${category}/`) && path.endsWith('.md') && !path.includes('.sync-conflict-')).length;
      for (const category of ['Projects', 'Areas', 'Resources', 'Archives']) {
        if (counts[category] !== ordinary(category)) fail(`${side.name} ${category} counts ${counts[category]}, but holds ${ordinary(category)} ordinary notes`);
      }
      const graphView = await walkthrough.graphNodes(side.browser);
      if (!graphView.graph.nodes) fail(`${side.name} graph data failed: ${JSON.stringify(graphView.graph)}`);
      const graphPaths = graphView.graph.nodes.map((node) => node.path);
      if (graphPaths.some((path) => path.includes('sync-conflict'))) fail(`${side.name} graph holds a conflict copy`);
      const ordinaryTotal = ['Projects', 'Areas', 'Resources', 'Archives'].reduce((sum, category) => sum + ordinary(category), 0);
      // The panel's own count is the open note's neighbourhood while a note is open, so the
      // whole graph is counted from the data the panel draws.
      if (graphPaths.length !== ordinaryTotal) fail(`${side.name} graph holds ${graphPaths.length} notes, ${ordinaryTotal} ordinary`);
      const areaTitles = await walkthrough.categoryTitles(side.browser, 'Areas');
      if (areaTitles.some((title) => title.includes('sync-conflict'))) fail(`${side.name} lists a conflict copy as a note`);
      const hits = {};
      for (const { copyWord } of plan) {
        hits[copyWord] = await walkthrough.keywordTitles(side.browser, side.type, copyWord);
        if (hits[copyWord].length) fail(`${side.name} search finds the conflict-only word ${copyWord}: ${JSON.stringify(hits[copyWord])}`);
      }
      exclusion[side.name] = { counts, graphNotes: graphPaths.length, graphPanel: graphView.stats, conflictOnlySearchHits: 0 };
      await shot(side, 'c8-conflicts');
    }
    // #191: both machines keep the "use" note open through the choice and the sync that follows.
    const useItem = plan.find((item) => item.note.name === 'use');
    for (const side of sides) await walkthrough.openNoteIn(side.browser, useItem.note.category, useItem.note.title);
    const resolved = [];
    for (const item of plan) resolved.push(await walkthrough.resolveConflict(resolver.browser, item.entry.relativePath, item.choice));
    await closeSettings(resolver);
    await syncBatch('conflict-resolved', {
      mark: markBackups(),
      until: () => sides.every((side) => {
        const summary = side.machine.vaultSummary(plan.flatMap((item) => [item.copyWord, item.currentWord]));
        return !summary.conflicts.length && plan.every((item) => {
          const kept = item.choice === 'Keep current' ? item.currentWord : item.copyWord;
          const dropped = item.choice === 'Keep current' ? item.copyWord : item.currentWord;
          return summary.markers[kept].includes(pathOf(item.note)) && !summary.markers[dropped].includes(pathOf(item.note))
            && summary.markers[dropped].some((path) => path.startsWith('.helixnotes/trash/'));
        });
      }),
    });
    // Each open editor shows the chosen version without a reopen: the resolver's after its own
    // choice, the receiver's after the sync. An edit then saves, so the editor took the new
    // revision. The receiver's edit waits for one more batch to bring it the resolver's edit,
    // so the two edits never conflict.
    const showsChosen = async (side, words) => {
      let shown = null;
      await side.browser.waitUntil(async () => {
        shown = { ...(await editorState(side)), text: await walkthrough.editorText(side.browser) };
        return shown.title === useItem.note.title && !shown.dirty && words.every((word) => shown.text.includes(word)) && !shown.text.includes(useItem.currentWord);
      }, { timeout: 30_000, interval: 1_000 }).catch(() => fail(`${side.name} editor does not show the chosen version: ${JSON.stringify({ ...shown, text: shown?.text.slice(0, 300) })}`));
      return shown;
    };
    const editAfterChoice = async (side) => {
      const word = marker(`after${side.name.slice(0, 3)}`);
      await walkthrough.appendToNote(side.browser, side.type, ` After the choice ${word}.`);
      await waitForFile(side, Object.assign((summary) => summary.markers[word].includes(pathOf(useItem.note)), { markers: [word] }), `${side.name} edit after the choice saved`);
      return word;
    };
    const receiver = peerOf(resolver);
    const openEditors = {};
    for (const side of sides) await showsChosen(side, [useItem.copyWord]);
    const resolverWord = await editAfterChoice(resolver);
    await syncBatch('after-choice-edit', {
      mark: markBackups(),
      until: () => receiver.machine.vaultSummary([resolverWord]).markers[resolverWord].includes(pathOf(useItem.note)),
    });
    await showsChosen(receiver, [useItem.copyWord, resolverWord]);
    const receiverWord = await editAfterChoice(receiver);
    openEditors[resolver.name] = { showedChoiceWithoutReopen: true, editSaved: resolverWord };
    openEditors[receiver.name] = { showedChoiceWithoutReopen: true, showedSyncedEditWithoutReopen: true, editSaved: receiverWord };
    return { exclusion, resolved: plan.map((item) => ({ path: pathOf(item.note), choice: item.choice, chosenOnBoth: true, discardedInTrash: true })), resolvedOn: resolver.name, messages: resolved.map((item) => item.message), openEditors };
  });
  criterion(8, 'A conflicting edit surfaces both versions for the user to choose; the conflict copy never appears as an ordinary note in search, the graph, or PARA counts', { conflict });

  // ── #189: closing during a sync run exits inside the close handshake's 10 s ──
  // Each machine closes once while its run waits for a peer that has quit, and once during the
  // handoff hold after both runs confirmed. The note has an edit the run's lease is holding back,
  // so the close has to stop the run for that save to land.
  const closeDuringSync = await step('close-during-sync', async () => {
    const results = [];
    const folderPath = `/rest/config/folders/${encodeURIComponent(vaultId)}`;
    const reopen = async (side) => {
      side.browser = await openApp(side.machine.port, side.machine.application);
      await instrument(side);
    };
    for (const side of sides) {
      const peer = peerOf(side);
      for (const phase of ['peer-wait', 'handoff']) {
        const label = `${phase}-${side.name}`;
        const word = marker(`close${label}`);
        const edited = marker(`edit${label}`);
        const note = { category: 'Projects', title: `Close ${label} ${runId}`, body: `Close test ${word}.` };
        const notePath = `${note.category}/${note.title}.md`;
        await walkthrough.createNote(side.browser, side.type, note);
        await waitForFile(side, Object.assign((summary) => summary.markers[word].includes(notePath), { markers: [word] }), `${label} note saved`);
        if (phase === 'peer-wait') {
          await walkthrough.closeWindow(peer.browser);
          await peer.machine.appsGone();
          await closeApp(peer.browser);
        }
        const pressedAt = Date.now();
        await walkthrough.pressSyncNow(side.browser);
        if (phase === 'handoff') await walkthrough.pressSyncNow(peer.browser);
        await waitForState(async () => (await side.machine.syncthing('GET', folderPath)).paused, (paused) => paused === false, 30_000, `${side.name} batch running`);
        let batch = { folderPaused: false };
        if (phase === 'handoff') {
          // Both runs confirm after 15 s of clean observations, then hold for up to 60 s.
          await sleep(Math.max(0, pressedAt + 25_000 - Date.now()));
          const completion = await side.machine.syncthing('GET', `/rest/db/completion?folder=${encodeURIComponent(vaultId)}&device=${identities[peer.name].deviceId}`);
          const done = await side.browser.execute(() => window.__syncDone);
          if (completion.completion !== 100 || done !== null) fail(`${side.name} is not in the handoff hold: ${JSON.stringify({ completion: completion.completion, done })}`);
          batch = { ...batch, peerCompletion: completion.completion };
        }
        await closeSettings(side);
        await walkthrough.editNote(side.browser, side.type, { category: note.category, title: note.title, text: ` Edited during the run ${edited}.` });
        const before = await editorState(side);
        if (!before.dirty) fail(`${label}: the edit saved during the run, so the run held no lease`);
        const closedAt = Date.now();
        await walkthrough.closeWindow(side.browser);
        await waitForState(() => side.machine.syncRoles(), (rows) => !rows.some((row) => row.role === 'App'), 10_000, `${label}: app exit within 10 s of close`)
          .catch(async (error) => {
            record('close-stuck', side.name, await editorState(side));
            await shot(side, `${label}-close-stuck`);
            throw error;
          });
        const exitSeconds = (Date.now() - closedAt) / 1000;
        await side.machine.appsGone();
        await closeApp(side.browser);
        const saved = side.machine.vaultSummary([edited]).markers[edited].includes(notePath);
        if (!saved) fail(`${label}: the edit made during the run was not saved before exit`);
        await reopen(side);
        if (phase === 'peer-wait') await reopen(peer);
        results.push({ label, exitSeconds, dirtyAtClose: before.dirty, saved, ...batch });
      }
    }
    for (const side of sides) await walkthrough.vaultOpened(side.browser);
    const words = sides.flatMap((side) => ['peer-wait', 'handoff'].map((phase) => marker(`edit${phase}-${side.name}`)));
    await syncBatch('after-close-tests', {
      mark: markBackups(),
      until: () => sides.every((side) => {
        const summary = side.machine.vaultSummary(words);
        return words.every((word) => summary.markers[word].some((path) => path.startsWith('Projects/')));
      }),
    });
    return { results, convergedAfter: true };
  });
  record('close-during-sync', 'controller', { issue: 189, ...closeDuringSync });

  // ── 10. No internet (Fedora offline; Windows app isolated) and no Notion ──
  const offline = await step('offline', async () => {
    const peerIp = identities.windows.tailscaleIp;
    const clipBefore = await walkthrough.clipPage(W.browser, W.type, { url: ACCEPTANCE_CLIP_BEFORE, category: 'Resources', expectedText: 'Zettelkasten' });
    // Both HTTPS probes have to work, or "fails during" proves nothing; one retry covers a slow
    // public host.
    const online = async () => {
      let probe = network.probe(peerIp);
      if (typeof probe.httpsByAddress !== 'number' || typeof probe.httpsByName !== 'number') {
        await sleep(5_000);
        probe = { ...network.probe(peerIp), retried: true };
      }
      if (typeof probe.httpsByAddress !== 'number' || typeof probe.httpsByName !== 'number') fail(`Fedora is not online: ${JSON.stringify(probe)}`);
      return probe;
    };
    const before = { fedora: await online(), windowsAppClip: clipBefore.title };
    const isolation = { fedora: network.isolate(), windows: W.machine.isolation('isolate') };
    const programs = isolation.windows.programs.map((path) => path.split('\\').pop().toLowerCase());
    if (!['second-brain.exe', 'syncthing.exe', 'msedgewebview2.exe'].every((name) => programs.includes(name))) fail(`Windows isolation misses a program: ${programs}`);
    const during = { fedora: network.probe(peerIp) };
    if (during.fedora.publicRoute4 || during.fedora.publicRoute6 || typeof during.fedora.httpsByAddress === 'number' || typeof during.fedora.httpsByName === 'number') fail(`Fedora still reaches the internet: ${JSON.stringify(during.fedora)}`);
    if (!/^[\d.]+:\d+$/.test(during.fedora.tailscalePath)) fail(`Tailscale is not on a direct path: ${during.fedora.tailscalePath}`);
    const clipDuring = await walkthrough.clipFails(W.browser, W.type, { url: ACCEPTANCE_CLIP_DURING, category: 'Resources' });
    if (!/could not be reached|took too long/.test(clipDuring.message)) fail(`the isolated Windows app clip failed for another reason: ${clipDuring.message}`);
    during.windowsAppClip = clipDuring.message;
    const notion = {};
    for (const side of sides) notion[side.name] = { tokenStored: side.machine.notionTokenStored() };
    if (Object.values(notion).some((value) => value.tokenStored)) fail(`a Notion token is stored: ${JSON.stringify(notion)}`);
    const trips = [];
    const offlineTrip = async (from, to, letter) => {
      const image = { name: `offline-${letter}-${tag}.png`, bytes: solidPng(32, 32, letter === 'w' ? [30, 60, 220] : [230, 180, 20]) };
      const sent = await sendAttachments(from, to, `offline-${letter}`, { image });
      await syncBatch(`offline-${letter}`, {
        mark: sent.mark, receiver: to, prior: sent.prior,
        until: () => {
          const { files } = to.machine.vaultSummary();
          return Boolean(files[sent.notePath]) && files[sent.paths[image.name]] === sent.sent.files[sent.paths[image.name]];
        },
      });
      const titles = await walkthrough.keywordTitles(to.browser, to.type, sent.word);
      if (!titles.includes(sent.note.title)) fail(`${to.name} search did not find "${sent.note.title}" offline: ${JSON.stringify(titles)}`);
      await walkthrough.openNoteIn(to.browser, sent.note.category, sent.note.title);
      const rendered = await expectRendered(to, sent.paths[image.name], 'offline image');
      const sockets = W.machine.connections();
      const outside = sockets.connections.filter((socket) => !/^(100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\.|127\.|0\.0\.0\.0$|::$|::1$|fd7a:115c:a1e0:)/i.test(socket.remote));
      if (outside.length) fail(`Windows app stack holds non-Tailnet connections: ${JSON.stringify(outside)}`);
      await shot(to, `c10-offline-${letter}`);
      trips.push({ from: from.name, to: to.name, note: sent.notePath, searchFound: true, imageWidth: rendered.naturalWidth, windowsSockets: sockets.connections.length, marker: sent.word });
    };
    await offlineTrip(W, F, 'w');
    await offlineTrip(F, W, 'f');
    // Still isolated at the end, or the trips above prove nothing.
    const end = { fedora: network.probe(peerIp), windows: W.machine.isolation('state') };
    if (end.fedora.publicRoute4 || typeof end.fedora.httpsByAddress === 'number' || end.windows.rules.length !== isolation.windows.rules.length) fail(`isolation ended before the trips finished: ${JSON.stringify(end)}`);
    const restored = { fedora: await network.restore(), windows: W.machine.isolation('restore') };
    const after = { fedora: await online() };
    after.windowsAppClip = (await walkthrough.clipPage(W.browser, W.type, { url: ACCEPTANCE_CLIP_AFTER, category: 'Resources', expectedText: 'ommonplace' })).title;
    return { before, isolation: { fedora: isolation.fedora, windows: { programs, rules: isolation.windows.rules.length } }, during, notion, trips, end: { windowsRules: end.windows.rules.length }, restored, after };
  });
  criterion(10, 'Sync works with no internet (Tailnet only) and with no Notion account configured', {
    fedora: 'host offline: no public IPv4/IPv6 route, HTTPS by address and by name failed',
    windows: 'application-isolated: app, sidecar, and WebView2 blocked from every non-Tailnet address',
    trips: offline.trips, notion: offline.notion,
  });

  // ── 9. Backups ──
  criterion(9, 'A vault backup is forced before a batch of incoming changes is applied', {
    rule: 'every batch that brought changes left a new pre-sync backup on its receiver holding the receiver\'s previous state of the changed paths; see the sync-batch lines',
  });
  // ── 11. Both real machines ──
  criterion(11, 'Verified by running it on both real machines, not simulated with two local folders', {
    machines: ['Fedora 44 laptop (installed RPM)', 'Windows 11 desktop (installed NSIS package)'], transport: 'Tailscale direct path between the two machines',
  });

  async function deleteThroughMenu(side, note) {
    await walkthrough.openCategory(side.browser, note.category);
    await walkthrough.openNoteIn(side.browser, note.category, note.title);
    await side.browser.execute((title) => {
      const row = [...document.querySelectorAll('.note-title')].find((element) => element.innerText.trim() === title);
      row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 200, clientY: 200 }));
    }, note.title);
    const pressed = await side.browser.execute(() => {
      const button = [...document.querySelectorAll('button')].find((element) => element.innerText.trim() === 'Move to Trash');
      button?.click();
      return Boolean(button);
    });
    if (!pressed) fail('no Move to Trash in the note menu');
    await waitForFile(side, (summary) => !summary.files[`${note.category}/${note.title}.md`], `${note.title} trashed`);
  }
}

async function main() {
  const [commandName, ...args] = process.argv.slice(2);
  if (commandName === 'sync') return runSync(args);
  if (commandName === 'restore') return runRestore(args);
  if (commandName === 'walkthrough') return runWalkthrough(args);
  if (commandName === 'acceptance') return runAcceptance(args);
  if (commandName === 'walkthrough-uninstalled') return runWalkthroughUninstalled(args);
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
  console.error([
    'usage: node scripts/alpha-harness.mjs sync|restore [--candidate <sha>] [--ssh sb-windows] [--linux-root ~/sb88] [--timeout-minutes 20]',
    '       node scripts/alpha-harness.mjs walkthrough --windows-installer <D:\\...setup.exe> --fedora-rpm <rpm> [--candidate <sha>] [--machine fedora|windows] [--ollama-port 11434]',
    '       node scripts/alpha-harness.mjs walkthrough-uninstalled --run <runId>',
    '       node scripts/alpha-harness.mjs acceptance --fedora-rpm <rpm> [--candidate <sha>]',
  ].join('\n'));
  process.exitCode = 2;
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exitCode = 1;
  });
}
