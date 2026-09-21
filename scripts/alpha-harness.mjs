#!/usr/bin/env node

import { createHash, randomUUID } from 'node:crypto';
import {
  appendFileSync,
  closeSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readlinkSync,
  readdirSync,
  renameSync,
  rmdirSync,
  statfsSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { homedir } from 'node:os';
import { isIP } from 'node:net';
import { dirname, join, resolve, win32 } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { check, generate, NOTE_COUNT } from './sync-fixture.mjs';

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const REPO = resolve(dirname(SCRIPT_PATH), '..');
const WINDOWS_TOOLS = win32.join('D:\\SecondBrainTest\\sb88', 'tools');
const WINDOWS_ROOT = 'D:\\SecondBrainTest\\sb88';
const WINDOWS_APP = 'D:\\SecondBrainTest\\app\\second-brain.exe';
const LINUX_APP = '/usr/bin/second-brain';
const MIN_FREE_BYTES = 5 * 1024 ** 3;
const DEFAULT_TIMEOUT_MS = 20 * 60_000;
const WINDOWS_TASK = 'SecondBrainAlphaHarness';
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

export function harnessConfig(config, vaultPath, vaultId, backupPath) {
  const next = structuredClone(config);
  next.vaults = [{ path: vaultPath, name: 'Alpha Harness', vault_id: vaultId }];
  next.active_vault = vaultPath;
  next.active_bookmark_id = null;
  next.backup_location = backupPath;
  next.backup_max_count = Math.max(10, Number(next.backup_max_count) || 0);
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

function prepareLinux(root, runId, vaultId) {
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
  const control = makeControl(vaultId);
  atomicWrite(join(machinePath, 'sync-control.json'), `${JSON.stringify(control, null, 2)}\n`);
  atomicWrite(
    configPath,
    `${JSON.stringify(harnessConfig(localTemplateConfig(), vaultPath, vaultId, backupPath), null, 2)}\n`,
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

function remoteWorker(sshHost, request) {
  const remoteScript = win32.join(WINDOWS_TOOLS, 'alpha-harness.mjs');
  const result = runCommandSync('ssh', [
    sshHost,
    'node',
    remoteScript,
    '__windows-worker',
    base64Request(request),
  ], { timeout: 45_000 });
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
      let processesStopped = false;
      if (windowsPid) {
        try {
          remoteWorker(options.sshHost, { action: 'stop-app', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP, pid: windowsPid });
        } catch (error) {
          observation(evidencePath, 'cleanup-error', 'windows-app', { error: error.message });
          cleanupError ??= error;
        }
      }
      try {
        let finalReport;
        try {
          finalReport = await waitForWindowsProcesses(options.sshHost, runId, (processes) => processes.length === 0, 60_000);
        } catch (error) {
          const forced = remoteWorker(options.sshHost, { action: 'cleanup-processes', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP });
          observation(evidencePath, 'forced-process-cleanup', 'windows', forced);
          finalReport = await waitForWindowsProcesses(options.sshHost, runId, (processes) => processes.length === 0, 15_000);
          cleanupError ??= error;
        }
        observation(evidencePath, 'process-final', 'windows', finalReport);
        processesStopped = true;
      } catch (error) {
        observation(evidencePath, 'cleanup-error', 'windows-processes', { error: error.message });
        cleanupError ??= error;
      }
      if (processesStopped) {
        try {
          const finalized = remoteWorker(options.sshHost, { action: 'finalize', root: WINDOWS_ROOT, runId, appPath: WINDOWS_APP });
          observation(evidencePath, 'config-restored', 'windows', finalized);
        } catch (error) {
          observation(evidencePath, 'cleanup-error', 'windows-config', { error: error.message });
          cleanupError ??= error;
        }
      }
    }
  }

  if (runError) throw runError;
  if (cleanupError) throw cleanupError;
  if (!completed) fail(`run failed; evidence: ${evidencePath}`);
  observation(evidencePath, 'run-complete', 'controller');
  console.log(JSON.stringify({ runId, evidencePath }));
}

function windowsPaths(root, runId) {
  const runRoot = win32.join(root, 'runs', runId, 'windows');
  return {
    runRoot,
    vaultPath: win32.join(runRoot, 'vault'),
    backupPath: win32.join(runRoot, 'backups'),
    machinePath: win32.join(runRoot, 'machine-state'),
    manifestPath: win32.join(runRoot, 'state.json'),
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

    const control = makeControl(request.vaultId);
    atomicWrite(win32.join(paths.machinePath, 'sync-control.json'), `${JSON.stringify(control, null, 2)}\n`);
    const lockPath = win32.join(dirname(configPath), 'alpha-harness.lock.json');
    const manifest = {
      version: 1, runId: request.runId, startedAt: new Date().toISOString(), configPath, originalPath, machineLink, appPath,
    };
    atomicWrite(paths.manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    writeFileSync(lockPath, `${JSON.stringify({ runId: request.runId, manifestPath: paths.manifestPath })}\n`, { flag: 'wx' });
    atomicWrite(
      configPath,
      `${JSON.stringify(harnessConfig(original, paths.vaultPath, request.vaultId, paths.backupPath), null, 2)}\n`,
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
  if (commandName === '__windows-worker') {
    const request = JSON.parse(Buffer.from(args[0], 'base64url').toString('utf8'));
    const result = await windowsWorker(request);
    console.log(JSON.stringify(result));
    return;
  }
  console.error('usage: node scripts/alpha-harness.mjs sync [--candidate <sha>] [--ssh sb-windows] [--linux-root ~/sb88] [--timeout-minutes 20]');
  process.exitCode = 2;
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exitCode = 1;
  });
}
