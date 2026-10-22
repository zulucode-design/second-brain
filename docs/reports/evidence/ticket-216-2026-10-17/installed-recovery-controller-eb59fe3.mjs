// Run from the repository root: SB216_RUN_ROOT=<private root> SB216_SSH_TARGET=<alias>
// node --input-type=module < docs/reports/evidence/ticket-216-2026-10-17/installed-recovery-controller-eb59fe3.mjs
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, readlinkSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, join } from 'node:path';
import { remote } from 'webdriverio';
import { invoke, sleep, waitForIndex, waitForPort } from './scripts/ask-run.mjs';

const root = join(process.env.SB216_RUN_ROOT, 'manual');
const appId = 'io.github.zulucodedesign.SecondBrain';
const app = '/usr/bin/second-brain';
const candidate = 'eb59fe3db78591d0ae10bfccffc0fe24ee1d77e9';
const fixture = JSON.parse(readFileSync('scripts/similarity-fixture.json', 'utf8'));
const trace = { candidate, started: new Date().toISOString(), steps: [], result: 'RUNNING' };
const children = [];
let browser = null;
let busOutput = '';
let appPid = null;
let launchCount = 0;
const save = () => writeFileSync(join(root, 'observations.json'), JSON.stringify(trace, null, 2) + '\n');
const observe = (step, evidence) => {
  trace.steps.push({ step, at: new Date().toISOString(), ...evidence });
  if (step !== 'safety' && existsSync(deferredPath)) {
    const snapshot = JSON.parse(readFileSync(deferredPath, 'utf8'));
    snapshot.checks = snapshot.checks.map((record) => ({ ...record, path: record.path.replace(vault, '<fixture-vault>') }));
    writeFileSync(join(root, step + '-deferred.json'), JSON.stringify(snapshot, null, 2) + '\n');
  }
  save(); console.log(step);
};
const requireThat = (condition, message) => { if (!condition) throw new Error(message); };
const walk = (dir) => !existsSync(dir) ? [] : readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
  const path = join(dir, entry.name);
  return entry.isDirectory() ? walk(path) : [path];
});
const runningApps = () => readdirSync('/proc').filter((name) => /^\d+$/.test(name)).flatMap((name) => {
  try { return readlinkSync('/proc/' + name + '/exe') === app ? [Number(name)] : []; } catch { return []; }
});
const waitFor = async (predicate, label, timeout = 60_000) => {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) { if (await predicate()) return; await sleep(500); }
  throw new Error('Timed out: ' + label);
};
const profilePaths = ['.config', '.local/share', '.local/state', '.cache'].flatMap((folder) =>
  [appId, 'helixnotes'].map((name) => join(homedir(), folder, name)));
const profileManifest = () => profilePaths.map((path) => ({
  path, exists: existsSync(path),
  files: walk(path).map((file) => ({
    relative: file.slice(path.length + 1),
    sha256: createHash('sha256').update(readFileSync(file)).digest('hex'),
  })).sort((a, b) => a.relative.localeCompare(b.relative)),
}));
const configHome = join(root, 'config');
const dataHome = join(root, 'data');
const vault = join(root, 'vault');
const vaultId = randomUUID();
const configPath = join(configHome, appId, 'config.json');
const deferredPath = join(dataHome, appId, 'vaults', vaultId, 'deferred_similarity.json');
const checks = () => JSON.parse(readFileSync(deferredPath, 'utf8')).checks;
const publicChecks = () => checks().map((record) => ({ id: record.id, file: basename(record.path) }));
const logText = () => walk(dataHome).filter((file) => file.endsWith('.log')).map((file) => readFileSync(file, 'utf8')).join('\n');
const skipped = () => logText().split('Skipped the similarity check for a new note').length - 1;
const notifications = () => busOutput.split(/^(?=method (?:call|return) )/m).filter((message) => {
  const values = [...message.matchAll(/^\s+(string|uint32) "?(.*?)"?$/gm)].map((match) => match[2]);
  return message.startsWith('method call ') && message.includes('member=Notify') && values[0] === 'Second Brain';
}).length;
let originalProfile = null;
let config = null;

async function launch() {
  requireThat(runningApps().length === 0, 'An installed app is already running');
  browser = await remote({
    hostname: '127.0.0.1', port: 4456, logLevel: 'error', connectionRetryCount: 0,
    capabilities: { 'tauri:options': { application: app } },
    waitforTimeout: 60_000, connectionRetryTimeout: 60_000,
  });
  await browser.setTimeout({ script: 30_000 });
  launchCount += 1;
  await browser.setWindowSize(1280, 860);
  await browser.$('button=New Note').waitForDisplayed({ timeout: 60_000 });
  const apps = runningApps();
  requireThat(apps.length === 1, 'Expected exactly one installed app process');
  appPid = apps[0];
  const health = await browser.execute(() => ({ ready: document.readyState, tauri: typeof window.__TAURI_INTERNALS__?.invoke === 'function', newNote: [...document.querySelectorAll('button')].some((b) => b.innerText.trim() === 'New Note') }));
  requireThat(health.ready === 'complete' && health.tauri && health.newNote, 'Fresh app session failed health check');
  const actualConfig = await invoke(browser, 'get_app_config');
  requireThat(!actualConfig?.failed && actualConfig.ollama_base_url === config.ollama_base_url, 'App backend URL differs from isolated config');
  requireThat(actualConfig.active_vault === vault && actualConfig.backup_enabled === false, 'App isolation settings differ');
  await browser.saveScreenshot(join(root, 'health-session-' + launchCount + '.png'));
  trace.sessions ??= [];
  trace.sessions.push({launch: launchCount, pid: appPid, health, backendUrl: actualConfig.ollama_base_url});
  save();
}
async function quit() {
  if (!browser) return;
  const closing = browser;
  await closing.execute(() => {
    void window.__TAURI_INTERNALS__.invoke('plugin:window|close', { label: 'main' });
    return true;
  });
  await waitFor(() => !existsSync('/proc/' + appPid), 'app graceful exit', 30_000);
  await closing.deleteSession().catch(() => {});
  browser = null;
  appPid = null;
}

try {
  requireThat(!existsSync(root), 'Manual run directory already exists');
  requireThat(runningApps().length === 0, 'An installed app is already running');
  mkdirSync(root);
  originalProfile = profileManifest();
  mkdirSync(join(root, 'profile-backup'));
  for (let index = 0; index < profilePaths.length; index += 1) {
    if (existsSync(profilePaths[index])) cpSync(profilePaths[index], join(root, 'profile-backup', String(index)), { recursive: true, dereference: false });
  }
  writeFileSync(join(root, 'profile-backup', 'manifest.private.json'), JSON.stringify(originalProfile, null, 2));
  observe('safety', { profileRoots: profilePaths.length, presentRoots: originalProfile.filter((entry) => entry.exists).length, isolated: true });

  const tunnel = spawn('ssh', ['-N', '-o', 'ExitOnForwardFailure=yes', '-L', '127.0.0.1:11436:127.0.0.1:11436', process.env.SB216_SSH_TARGET], { stdio: 'ignore' });
  children.push(tunnel);
  trace.tunnelPid = tunnel.pid;
  await waitForPort(11436, tunnel, 'gate-owned embedding tunnel');
  const tags = await fetch('http://127.0.0.1:11436/api/tags').then((response) => response.json());
  requireThat(tags.models.some((model) => model.name.startsWith('embeddinggemma')), 'Embedding model is unavailable');
  const closedProbe = await fetch('http://127.0.0.1:11447/api/tags').then(() => true, () => false);
  requireThat(!closedProbe, 'Offline port is not closed');

  for (const category of ['Projects', 'Areas', 'Resources', 'Archives']) mkdirSync(join(vault, category), { recursive: true });
  mkdirSync(join(vault, '.helixnotes'));
  writeFileSync(join(vault, '.helixnotes', 'vault_id'), vaultId);
  for (let index = 0; index < 3; index += 1) {
    const note = fixture.notes[index];
    const id = '00000009-0000-4000-8000-' + String(index + 1).padStart(12, '0');
    writeFileSync(join(vault, note.category, note.title + '.md'),
      '---\nid: "' + id + '"\ntitle: "' + note.title + '"\ntags: []\npinned: false\ncreated: 2026-10-05T00:00:00+00:00\nmodified: 2026-10-05T00:00:00+00:00\ncategory: ' + note.category + '\n---\n' + note.body + '\n');
  }
  mkdirSync(join(configHome, appId), { recursive: true });
  mkdirSync(dataHome);
  config = {
    theme: 'system', vault: { path: vault, name: 'Similarity fixture', vault_id: vaultId }, active_vault: vault,
    ai_provider: 'ollama', ai_model: 'gpt-oss:20b-cloud', ai_models: { ollama: 'gpt-oss:20b-cloud' },
    ollama_base_url: 'http://127.0.0.1:11447', backup_enabled: false, close_to_tray: false,
  };
  writeFileSync(configPath, JSON.stringify(config, null, 2));
  const monitor = spawn('dbus-monitor', ['--session', "type='method_call',interface='org.freedesktop.Notifications',member='Notify'"], { stdio: ['ignore', 'pipe', 'ignore'] });
  children.push(monitor);
  monitor.stdout.on('data', (chunk) => { busOutput += chunk; });
  await waitFor(() => monitor.exitCode === null && busOutput.length > 0, 'notification monitor ready', 10_000);
  trace.notificationMonitorPid = monitor.pid;
  trace.notificationMonitorReady = true;
  const driver = spawn('tauri-driver', ['--port', '4456', '--native-port', '4457'], {
    env: { ...process.env, XDG_CONFIG_HOME: configHome, XDG_DATA_HOME: dataHome, XDG_STATE_HOME: join(root, 'state'), XDG_CACHE_HOME: join(root, 'cache') },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  children.push(driver);
  let driverOutput = '';
  driver.stdout.on('data', (chunk) => { driverOutput += chunk; });
  driver.stderr.on('data', (chunk) => { driverOutput += chunk; });
  driver.on('exit', () => writeFileSync(join(root, 'driver.private.log'), driverOutput));
  await waitForPort(4456, driver, 'tauri-driver');
  await launch();
  const captured = [];
  for (const item of fixture.captures.slice(0, 2)) {
    const entry = await invoke(browser, 'quick_capture_note', { category: 'Areas', title: item.title, body: item.body });
    requireThat(entry && !entry.failed, 'Quick capture failed');
    requireThat(existsSync(entry.path), 'Quick capture was not saved');
    captured.push(entry);
  }
  await waitFor(() => skipped() === 2 && existsSync(deferredPath) && checks().length === 2, 'two skipped checks and two durable records');
  requireThat(await browser.execute(() => document.querySelectorAll('.similar-card').length) === 0, 'Offline capture displayed a card');
  observe('1-offline-captures', { captured: 2, skippedLogLines: skipped(), skipLogs: logText().split('\n').filter((line) => line.includes('Skipped the similarity check for a new note')),  records: publicChecks(), cards: 0, notificationCalls: notifications() });
  await quit();
  await launch();
  requireThat(checks().length === 2, 'Restart lost a deferred record');
  const deletion = await invoke(browser, 'delete_note', { path: captured[1].path });
  requireThat(!deletion?.failed && !existsSync(captured[1].path), 'In-app deletion failed');
  observe('2-restart-and-delete', { recordsAfterRestart: 2, deletedThroughAppCommand: true, sourceFileGone: true, recordsAfterDelete: publicChecks() });
  await quit();
  config.ollama_base_url = 'http://127.0.0.1:11436';
  writeFileSync(configPath, JSON.stringify(config, null, 2));
  observe('3-restore-backend', { reachableUrl: config.ollama_base_url, unreachableUrl: 'http://127.0.0.1:11447', isolatedConfigChangedWhileAppStopped: true, existingBackendUnchanged: true });
  await launch();
  const index = await waitForIndex(browser, 4);
  await waitFor(async () => await browser.execute(() => document.querySelectorAll('.similar-card').length) === 1, 'one deferred similarity card');
  const cards = await browser.execute(() => [...document.querySelectorAll('.similar-card')].map((card) => ({
    heading: card.querySelector('.similar-heading')?.innerText,
    matches: [...card.querySelectorAll('.similar-title span')].map((item) => item.innerText.trim()),
  })));
  requireThat(cards[0].heading.includes(captured[0].meta.title), 'Card names the wrong capture');
  requireThat(cards[0].matches.includes(fixture.captures[0].duplicateOf), 'Card omits the repeated fixture note');
  await waitFor(() => checks().length === 0 && logText().includes('Deferred similarity checks: ran=1 found=1 dropped=1 waiting=0'), 'settled deferred file and summary');
  await sleep(2_000);
  requireThat(notifications() === 0, 'App sent an OS notification');
  requireThat(monitor.exitCode === null, 'Notification monitor exited during the check');
  const finalTags = await fetch('http://127.0.0.1:11436/api/tags').then((response) => response.json());
  requireThat(finalTags.models.some((model) => model.name.startsWith('embeddinggemma')), 'Final forwarded endpoint check failed');
  trace.finalForwardedEndpoint = 'PASS';
  trace.notificationMonitorAliveAtFinalCheck = monitor.exitCode === null;
  await browser.saveScreenshot(join(root, 'similarity-card.png'));
  observe('4-recovered-card', { index, cards, log: 'Deferred similarity checks: ran=1 found=1 dropped=1 waiting=0', remainingRecords: checks().length, notificationCalls: notifications(), screenshot: 'similarity-card.png' });
  await quit();
  trace.result = 'PASS';
} catch (error) {
  trace.result = 'FAIL';
  trace.failure = String(error.message).replaceAll(root, '<manual-run>').replaceAll(homedir(), '<home>');
  console.error(error.stack);
  process.exitCode = 1;
} finally {
  if (browser) {
    try { await quit(); } catch (error) { trace.cleanupFailure = 'App did not quit gracefully'; process.exitCode = 1; trace.result = 'FAIL'; }
  }
  writeFileSync(join(root, 'notifications.private.log'), busOutput);
  for (const child of children.reverse()) {
    if (child.exitCode === null) {
      child.kill('SIGTERM');
      await Promise.race([new Promise((resolve) => child.once('exit', resolve)), sleep(5_000)]);
    }
  }
  if (originalProfile) {
    const unchanged = JSON.stringify(profileManifest()) === JSON.stringify(originalProfile);
    trace.profileUnchanged = unchanged;
    if (!unchanged) { trace.result = 'FAIL'; process.exitCode = 1; }
  }
  trace.remainingOwnedChildProcesses = children.filter((child) => child.exitCode === null && child.signalCode === null).map((child) => child.pid);
  if (trace.remainingOwnedChildProcesses.length) { trace.result = 'FAIL'; process.exitCode = 1; }
  trace.remainingAppProcesses = runningApps().length;
  if (trace.remainingAppProcesses) { trace.result = 'FAIL'; process.exitCode = 1; }
  trace.finished = new Date().toISOString();
  if (existsSync(root)) save();
  console.log('Manual result: ' + trace.result);
}
