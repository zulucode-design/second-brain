// The Gate 3 walkthrough plants a synthetic AI key that the app moves into the OS keyring (#252).
// It must refuse when the keyring already holds the entry, before it changes either machine, and
// remove what it planted after a pass, a failure or a signal, and only that. These tests run the
// walkthrough controller's own sequence (runWalkthroughLocked) against fake signals, fakes of
// Fedora's Secret Service commands and of the Windows desktop-session script, a fake Fedora machine,
// and the real Windows machine (windowsDriverMachine) and its real teardown over fake Windows
// actions. None touches a real keyring, machine or process.
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const {
  AI_KEY_ACCOUNTS, DESKTOP_ACTION_TIMEOUT_MS, DESKTOP_SCRIPT_TIMEOUT_MS, DESKTOP_TASK_BUDGET_MS, PLANTED_AI_KEYS,
  desktopCredentialAction, fedoraAiKeyStore, finishMachine, killEach, plantedAiKeyPatch, runDesktopScript, runWalkthroughLocked,
  windowsAiKeyAction, windowsAiKeyStore, windowsAiKeyTarget, windowsDriverMachine,
} = await import(new URL('../scripts/alpha-harness.mjs', import.meta.url));

const SERVICE = 'io.github.zulucodedesign.SecondBrain';
const TOOLS = 'D:\\SecondBrainTest\\sb88\\tools';
// Stand-ins for a user's real key, the key the walkthrough plants, and a Notion token the app
// stores on Connect. None may appear in anything the harness records, prints or throws.
const REAL_KEY = 'sk-real-user-key-0f3a9c11-do-not-read';
const PLANTED_KEY = 'sk-alpha-harness-5b2e7d90-planted';
const NOTION_TOKEN = 'notion-token-3e81-do-not-read';
const NOTION_ACCOUNT = 'integration:notion:fake-vault';
const PLATFORMS = ['fedora', 'windows'];
const SEARCH_PREFIX = [
  '--user', '--json=short', 'call', 'org.freedesktop.secrets', '/org/freedesktop/secrets',
  'org.freedesktop.Secret.Service', 'SearchItems', 'a{ss}', '2',
];

// Fedora's two commands, as the Secret Service answers them. `busctl ... SearchItems` lists the
// paths of the matching unlocked and locked items and never a secret; `secret-tool clear` deletes
// unlocked items only, prints nothing, and exits 1 with no message when no unlocked item matched.
// `locked` holds the accounts whose entry is locked. `searchFailsFrom` makes the nth search and
// later ones fail as a missing Secret Service does; `clearFails` makes the clear fail.
function fakeFedoraTools(entries, locked, log, { searchFailsFrom, clearFails = false } = {}) {
  let searches = 0;
  return (command, args, options) => {
    if (command === 'busctl') {
      assert.deepEqual(args.slice(0, SEARCH_PREFIX.length), SEARCH_PREFIX);
      const pairs = args.slice(SEARCH_PREFIX.length);
      assert.deepEqual(pairs.slice(0, 3), ['service', SERVICE, 'username']);
      assert.equal(pairs.length, 4);
      const account = pairs[3];
      log.push(`fedora:lookup ${account}`);
      searches += 1;
      if (searchFailsFrom && searches >= searchFailsFrom) {
        return { status: 1, stdout: '', stderr: 'Call failed: The name org.freedesktop.secrets was not provided by any .service files' };
      }
      const paths = entries.has(account) ? ['/org/freedesktop/secrets/collection/login/7'] : [];
      const data = locked.has(account) ? [[], paths] : [paths, []];
      return { status: 0, stdout: `${JSON.stringify({ type: 'aoao', data })}\n`, stderr: '' };
    }
    assert.equal(command, 'secret-tool');
    assert.deepEqual(options.stdio, ['ignore', 'ignore', 'pipe'], 'secret-tool output goes to the null device');
    const [verb, ...pairs] = args;
    assert.equal(verb, 'clear', 'existence never comes from secret-tool lookup');
    assert.deepEqual(pairs.slice(0, 3), ['service', SERVICE, 'username']);
    const account = pairs[3];
    log.push(`fedora:clear ${account}`);
    if (clearFails) return { status: 1, stdout: null, stderr: 'secret-tool: Cannot create an item in a locked collection' };
    if (!entries.has(account) || locked.has(account)) return { status: 1, stdout: null, stderr: '' };
    entries.delete(account);
    return { status: 0, stdout: null, stderr: '' };
  };
}

// alpha-desktop.ps1 in NotionToken mode, run as runDesktopScript runs it: the same target guard;
// found/left written to the result file; and for a Win32 error other than ERROR_NOT_FOUND, the
// call and its code written instead and a failed task, which runDesktopScript turns into a throw.
// `extra` models a script that writes more than it should; `appearsBetweenReadsFrom` an entry that
// appears between the script's two reads on its nth call and later ones.
function fakeDesktopScript(entries, log, { readErrorFrom, deleteError, extra = {}, appearsBetweenReadsFrom } = {}) {
  const files = new Map();
  const aiTargets = AI_KEY_ACCOUNTS.map((account) => `${account}.${SERVICE}`);
  const paths = [];
  let reads = 0;
  let calls = 0;
  return {
    files,
    paths,
    tools: TOOLS,
    runDesktop(scriptArguments) {
      assert.match(scriptArguments, /^-Mode NotionToken -OutputPath "[^"]+" -CredentialTarget "[^"]+"( -RemoveCredential)?$/);
      const outputPath = /-OutputPath "([^"]+)"/.exec(scriptArguments)[1];
      const target = /-CredentialTarget "([^"]+)"/.exec(scriptArguments)[1];
      const remove = scriptArguments.endsWith(' -RemoveCredential');
      assert.ok(outputPath.startsWith(`${TOOLS}\\`), 'the script writes only into the harness tools folder');
      paths.push(outputPath);
      if (!target.startsWith('integration:notion:') && !aiTargets.includes(target)) {
        throw new Error(`desktop script failed with 1: ${scriptArguments}`);
      }
      calls += 1;
      const account = target.slice(0, -(SERVICE.length + 1));
      const stopWith = (call, code) => {
        files.set(outputPath, JSON.stringify({ error: `${call} failed with Win32 error ${code}` }));
        throw new Error(`desktop script failed with 1: ${scriptArguments}`);
      };
      const read = () => {
        reads += 1;
        if (readErrorFrom && reads >= readErrorFrom) stopWith('CredReadW', 1312);
        return entries.has(account);
      };
      log.push(`windows:${remove ? 'clear' : 'lookup'} ${account}`);
      const found = read();
      if (found && remove) {
        if (deleteError) stopWith('CredDeleteW', deleteError);
        entries.delete(account);
      }
      if (appearsBetweenReadsFrom && calls >= appearsBetweenReadsFrom) entries.set(account, REAL_KEY);
      files.set(outputPath, JSON.stringify({ found, left: read(), ...extra }));
    },
    readResult(path) {
      if (!files.has(path)) throw new Error(`ENOENT: ${path}`);
      return JSON.parse(files.get(path));
    },
    discard: (path) => {
      files.delete(path);
    },
  };
}

// The keyring store for each platform, over its fake, with the Windows request and answer carried
// as JSON the way remoteWorker carries them.
function fakeStore(platform, entries, locked, log, faults) {
  if (platform === 'fedora') return fedoraAiKeyStore({ spawn: fakeFedoraTools(entries, locked, log, faults) });
  const desktop = fakeDesktopScript(entries, log, faults);
  const viaJson = (value) => JSON.parse(JSON.stringify(value));
  return windowsAiKeyStore((request) => viaJson(windowsAiKeyAction(viaJson(request), desktop)));
}

// The Fedora machine as linuxDriverMachine presents it to the walkthrough, logging each call. Its
// teardown only stops processes, since its config and state are the run's own folders.
// `app.alive` says whether its app could still write a credential: stopApps and cleanup stop it.
// `onLaunchEntry` models what the Start-menu launch may do to the keyring; `stopAppsFails` a stop
// whose final wait times out after the app has gone.
function fakeFedoraMachine(log, app, { onLaunchEntry, stopAppsFails = false } = {}) {
  const name = 'fedora';
  const stop = () => {
    app.alive = false;
    app.stopped();
  };
  return {
    name,
    application: `fake-${name}-app`,
    port: 0,
    prepare() {
      log.push(`${name}:prepare`);
      return { vault: `fake-${name}-vault` };
    },
    session: () => ({ locked: false, desktopSession: 1 }),
    async launchEntry() {
      log.push(`${name}:launch-entry`);
      onLaunchEntry?.();
      return {};
    },
    async startDriver() {
      log.push(`${name}:start-driver`);
      app.alive = true;
      return {};
    },
    patchConfig(patch) {
      log.push(`${name}:patch-config`);
      return Object.keys(patch);
    },
    breakConfig() {
      log.push(`${name}:break-config`);
    },
    hash: () => ({ sha256: 'unchanged' }),
    async stopApps() {
      log.push(`${name}:stop-apps`);
      stop();
      if (stopAppsFails) throw new Error('Fedora process survived app exit: []');
      return { processes: [] };
    },
    async cleanup() {
      log.push(`${name}:cleanup`);
      stop();
    },
  };
}

/**
 * What the real Windows worker and its process wait answer, for windowsDriverMachine's `io`:
 * `prepare` swaps the real config and takes the run's lock; `start-driver` starts the driver and,
 * through it, the app; `stop-driver` stops both; `cleanup-processes` stops the app; `finalize`
 * refuses while the app runs, as the real worker does, and otherwise restores the config and drops
 * the lock. `fail` makes the named actions, or `processes-gone`, fail for that many calls;
 * `appSurvivesDriverStop` leaves the app running after its driver stops.
 */
function fakeWindowsIo(world, { fail = {}, appSurvivesDriverStop = false, onLaunchEntry } = {}) {
  const { log, windows: state } = world;
  const app = world.apps.windows;
  const failures = { ...fail };
  const failing = (name) => {
    if (!failures[name]) return false;
    failures[name] -= 1;
    return true;
  };
  const stopApp = () => {
    if (!app.alive) return;
    app.alive = false;
    app.stopped();
  };
  return {
    worker(action, extra = {}) {
      log.push(`windows:${action}`);
      if (failing(action)) throw new Error(`ssh exited 255: the ${action} action could not reach Windows`);
      switch (action) {
        case 'install':
          return {};
        case 'prepare':
          state.configSwapped = true;
          state.locked = true;
          return { vault: 'fake-windows-vault' };
        case 'session':
          return { locked: false, desktopSession: 1 };
        case 'launch-entry':
          onLaunchEntry?.();
          return {};
        case 'start-driver':
          state.driverRunning = true;
          app.alive = true;
          return { pid: 4242 };
        case 'patch-config':
          return { patched: Object.keys(extra.patch) };
        case 'break-config':
          return {};
        case 'hash':
          return { sha256: 'unchanged' };
        case 'uninstall':
          return { appRemoved: true, shortcutRemoved: true };
        case 'stop-driver':
          assert.equal(extra.driverPid, 4242, 'the driver is stopped by the PID it started with');
          state.driverRunning = false;
          if (!appSurvivesDriverStop) stopApp();
          return { stopped: [4242] };
        case 'cleanup-processes':
          stopApp();
          return { before: [], after: [] };
        case 'finalize':
          if (app.alive) throw new Error('refusing to restore config while installed processes remain');
          state.configSwapped = false;
          state.locked = false;
          return { restored: true };
        default:
          throw new Error(`unexpected Windows action ${action}`);
      }
    },
    async processesGone(timeoutMs) {
      log.push(`windows:processes-gone ${timeoutMs}`);
      if (failing('processes-gone') || app.alive) throw new Error('Windows process state timed out: {"processes":[{"Role":"App"}]}');
      return { processes: [] };
    },
    tunnel: () => ({ exitCode: null, kill: () => log.push('windows:tunnel-killed') }),
    waitForDriver: async () => {},
  };
}

// The real Windows machine over fakeWindowsIo.
function fakeWindowsMachine(world, runId, faults) {
  return windowsDriverMachine('fake-host', runId, 'fake-vault-id', 'c'.repeat(40), { sync: true }, fakeWindowsIo(world, faults));
}

// Each platform's log line for the early stop a signal makes, and for its machine teardown.
const STOPPED = { fedora: 'fedora:stop-apps', windows: 'windows:cleanup-processes' };
const TORN_DOWN = { fedora: 'fedora:cleanup', windows: 'windows:finalize' };

/**
 * The walkthrough gate as walkthroughGate drives the machine up to and past the plant, ending as
 * `outcome` says. After the plant, the app's next launch files the key in the keyring. Its `wait`
 * outcomes hang the way a WebDriver wait does, until the test resumes it or the app is stopped;
 * `connect` models a Notion Connect whose request the test completes, after which a running app
 * stores its token. `afterPlant` changes the keyring after the plant; `breakTrace` makes every
 * later trace write fail.
 */
function fakeGate(world, { outcome = 'pass', patch = plantedAiKeyPatch(PLANTED_KEY), afterPlant, breakTrace = false } = {}) {
  const { entries, locked, log, apps } = world;
  const waitForApp = (name) => Promise.race([world.resumed, apps[name].gone]);
  return async (machine, { evidencePath }) => {
    try {
      machine.session();
      await machine.launchEntry();
      await machine.startDriver();
      if (outcome === 'fail-before-plant') throw new Error('the WebDriver session did not start');
      if (outcome === 'wait-before-plant') {
        world.reach();
        await waitForApp(machine.name);
      }
      machine.patchConfig(patch);
      entries[machine.name].set('ai:openai', PLANTED_KEY);
      log.push(`${machine.name}:app-filed-key`);
      afterPlant?.(entries[machine.name], locked[machine.name]);
      if (breakTrace) {
        rmSync(evidencePath);
        mkdirSync(evidencePath);
      }
      if (outcome === 'fail-after-plant') throw new Error('a step failed after the plant');
      if (outcome === 'wait-after-plant') {
        world.reach();
        await waitForApp(machine.name);
        machine.breakConfig();
      }
      if (outcome === 'keep-going-after-plant') {
        world.reach();
        await world.resumed;
        machine.breakConfig();
      }
      if (outcome === 'connect') {
        world.reach();
        await world.connected;
        log.push(`${machine.name}:connect-completed`);
        if (apps[machine.name].alive) entries[machine.name].set(NOTION_ACCOUNT, NOTION_TOKEN);
        machine.breakConfig();
      }
    } finally {
      log.push(`${machine.name}:gate-ended`);
    }
  };
}

function deferred() {
  let settle;
  const promise = new Promise((resolvePromise) => {
    settle = resolvePromise;
  });
  return { promise, settle };
}

/**
 * One controller run's world. `machine` is the --machine option. `held` puts a real key in a
 * platform's keyring before the run, `heldLocked` a locked one; `faults` (the keyring),
 * `machineFaults` (fakeFedoraMachine and fakeWindowsIo) and `launch` are per platform; `gate`
 * configures fakeGate; `deferFinal` holds the final Ollama check until the test finishes it.
 * `windows` is the Windows machine's state as the fake worker keeps it.
 */
function walkthroughWorld({ machine, held = [], heldLocked = [], faults = {}, machineFaults = {}, launch = {}, gate = {}, deferFinal = false } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'alpha-keyring-'));
  const log = [];
  const printed = [];
  const entries = { fedora: new Map(), windows: new Map() };
  const locked = { fedora: new Set(), windows: new Set() };
  for (const platform of [...held, ...heldLocked]) entries[platform].set('ai:openai', REAL_KEY);
  for (const platform of heldLocked) locked[platform].add('ai:openai');
  const apps = {};
  for (const platform of PLATFORMS) {
    const gone = deferred();
    apps[platform] = {
      alive: false,
      // A pending WebDriver wait ends when the app and driver it talks to are stopped.
      gone: gone.promise.then(() => {
        throw new Error('the app was stopped');
      }),
      stopped: gone.settle,
    };
    apps[platform].gone.catch(() => {});
  }
  const reached = deferred();
  const resumed = deferred();
  const connected = deferred();
  const finalReached = deferred();
  const finalAnswer = deferred();
  const signals = new EventEmitter();
  const world = {
    entries, locked, log, printed, signals, apps,
    windows: { configSwapped: false, locked: false, driverRunning: false },
    reached: reached.promise, reach: reached.settle,
    resumed: resumed.promise, resume: resumed.settle,
    connected: connected.promise, connect: connected.settle,
    finalReached: finalReached.promise, finishFinal: finalAnswer.settle,
  };
  const io = {
    signals,
    notion: () => ({ token: 'fake-notion-workspace-token', page: 'Fake page' }),
    commits() {
      log.push('commits');
      return { candidateCommit: 'c'.repeat(40), harnessCommit: 'h'.repeat(40) };
    },
    deployTools: () => log.push('deploy-tools'),
    evidencePath: (runId) => join(root, 'trace', `${runId}.jsonl`),
    keyStore: (name) => fakeStore(name, entries[name], locked[name], log, faults[name]),
    installCandidate: () => log.push('install-candidate'),
    prepareRun(early) {
      log.push('prepare-run');
      return { ...early, fedoraPackage: {}, recovered: { recovered: false } };
    },
    ollamaTunnel: () => ({ exitCode: null, kill: () => log.push('tunnel-killed') }),
    async ollamaModels(label) {
      log.push(`ollama-${label}`);
      if (label === 'final' && deferFinal) {
        finalReached.settle();
        await finalAnswer.promise;
      }
      return {};
    },
    makeMachine(name, runId) {
      const onLaunchEntry = launch[name] && (() => launch[name](entries[name], locked[name]));
      return name === 'fedora'
        ? fakeFedoraMachine(log, apps.fedora, { onLaunchEntry, ...machineFaults.fedora })
        : fakeWindowsMachine(world, runId, { onLaunchEntry, ...machineFaults.windows });
    },
    gate: fakeGate(world, gate),
    // The real Notion cleanup clears the run's Notion token from the keyring.
    async notionCleanup(target) {
      log.push(`${target.name}:notion-cleanup`);
      return { tokenFound: entries[target.name].delete(NOTION_ACCOUNT) };
    },
    sleep: async () => {},
    print: (line) => printed.push(line),
  };
  const options = { linuxRoot: root, holdMinutes: 70, windowsInstaller: 'D:\\SecondBrainTest\\setup.exe', machine };
  return Object.assign(world, {
    run: () => runWalkthroughLocked(options, io),
    traceText() {
      const traceDir = join(root, 'trace');
      return readdirSync(traceDir).map((name) => readFileSync(join(traceDir, name), 'utf8')).join('');
    },
    trace() {
      return this.traceText().trim().split('\n').map((line) => JSON.parse(line));
    },
    events(name) {
      return this.trace().filter((line) => line.event === name);
    },
    at(line) {
      return log.indexOf(line);
    },
    dispose: () => rmSync(root, { recursive: true, force: true }),
  });
}

// Everything a run could print or keep: the trace, what it printed, and its error.
function assertNoValue(world, error, { trace = true } = {}) {
  const text = `${trace ? world.traceText() : ''}\n${world.printed.join('\n')}\n${error?.message ?? ''}`;
  for (const value of [REAL_KEY, PLANTED_KEY, NOTION_TOKEN]) assert.ok(!text.includes(value), 'no secret is recorded, printed or thrown');
}

async function failure(promise) {
  try {
    await promise;
  } catch (error) {
    return error;
  }
  return assert.fail('the run was expected to fail');
}

// The run's failure, or a test failure if the run has not stopped within five seconds of a signal.
async function stopsWithin(running) {
  let timer;
  const late = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error('the run did not stop after the signal')), 5_000);
  });
  try {
    return await Promise.race([failure(running), late]);
  } finally {
    clearTimeout(timer);
  }
}

async function until(predicate, what) {
  const deadline = Date.now() + 5_000;
  while (!predicate()) {
    if (Date.now() > deadline) assert.fail(`timed out waiting for ${what}`);
    await new Promise((resolveLater) => setTimeout(resolveLater, 5));
  }
}

const changes = (log) => log.filter((line) => /install|prepare|launch|driver|patch|break|cleanup|clear|stop-apps|finalize/.test(line));

for (const platform of PLATFORMS) {
  test(`${platform}: an entry present before the run refuses it before the install and the recovery`, async (t) => {
    const world = walkthroughWorld({ machine: platform, held: [platform] });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, new RegExp(`keyring already holds .*${platform} \\(account ai:openai, service ${SERVICE.replaceAll('.', '\\.')}\\).*by hand`));
    assert.deepEqual(world.log, ['commits', 'deploy-tools', `${platform}:lookup ai:openai`], 'only reads, and the tools the check runs from');
    assert.deepEqual(changes(world.log), []);
    assert.equal(world.entries[platform].get('ai:openai'), REAL_KEY);
    assert.deepEqual(world.trace().map((line) => line.event), ['run-start', 'ai-keys-checked', 'run-failed']);
    assert.deepEqual(world.events('ai-keys-checked')[0].entries, { 'ai:openai': { existed: true } });
    assert.equal(world.signals.listenerCount('SIGINT') + world.signals.listenerCount('SIGTERM'), 0);
    assertNoValue(world, error);
  });

  test(`${platform}: a passing run plants the key and its cleanup deletes it once the app is stopped`, async (t) => {
    const world = walkthroughWorld({ machine: platform });
    t.after(world.dispose);

    await world.run();

    assert.equal(world.entries[platform].size, 0);
    assert.ok(world.at(`${platform}:lookup ai:openai`) < world.at('install-candidate'), 'the check comes before the install');
    assert.ok(world.at('install-candidate') < world.at('prepare-run'));
    // The re-check comes right before the patch, and the removal after the app and driver stopped.
    assert.equal(world.log[world.at(`${platform}:patch-config`) - 1], `${platform}:lookup ai:openai`);
    assert.ok(world.at(TORN_DOWN[platform]) < world.at(`${platform}:clear ai:openai`));
    assert.ok(!world.log.includes(STOPPED[platform]), 'a gate that settled on its own needs no early stop');
    assert.deepEqual(world.events('ai-keys-armed').map(({ armed, entries }) => ({ armed, entries })), [
      { armed: true, entries: { 'ai:openai': { existed: false } } },
    ]);
    assert.deepEqual(world.events('ai-keys-removed').map(({ machine, armed, entries }) => ({ machine, armed, entries })), [
      { machine: platform, armed: true, entries: { 'ai:openai': { removed: true, left: false } } },
    ]);
    assert.equal(world.events('run-complete').length, 1);
    assert.equal(world.printed.length, 1);
    assertNoValue(world);
  });

  test(`${platform}: an entry that appears after the check stops the run before the plant and deletes nothing`, async (t) => {
    // The Start-menu launch moves a key from the copied config into the keyring.
    const world = walkthroughWorld({ machine: platform, launch: { [platform]: (entries) => entries.set('ai:openai', REAL_KEY) } });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, new RegExp(`keyring gained an AI key entry .*${platform} \\(account ai:openai, service .*nothing was planted`));
    assert.ok(!world.log.includes(`${platform}:patch-config`), 'nothing is planted');
    assert.ok(!world.log.includes(`${platform}:clear ai:openai`), 'nothing is deleted');
    assert.equal(world.entries[platform].get('ai:openai'), REAL_KEY);
    assert.ok(world.log.includes(TORN_DOWN[platform]), 'the machine is still cleaned up');
    assert.deepEqual(world.events('ai-keys-armed').map(({ armed, entries }) => ({ armed, entries })), [
      { armed: false, entries: { 'ai:openai': { existed: true } } },
    ]);
    assert.deepEqual(world.events('ai-keys-removed').map(({ armed, entries }) => ({ armed, entries })), [{ armed: false, entries: undefined }]);
    assertNoValue(world, error);
  });

  test(`${platform}: a run that fails before the plant deletes nothing, even an entry its launch filed`, async (t) => {
    const world = walkthroughWorld({
      machine: platform,
      launch: { [platform]: (entries) => entries.set('ai:openai', REAL_KEY) },
      gate: { outcome: 'fail-before-plant' },
    });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, /the WebDriver session did not start/);
    assert.equal(world.entries[platform].get('ai:openai'), REAL_KEY);
    assert.ok(!world.log.includes(`${platform}:clear ai:openai`));
    assert.ok(world.log.includes(TORN_DOWN[platform]));
    assert.deepEqual(world.events('ai-keys-armed'), []);
    assert.deepEqual(world.events('ai-keys-removed').map(({ armed }) => armed), [false]);
    assertNoValue(world, error);
  });

  test(`${platform}: a run that fails after the plant still deletes the key`, async (t) => {
    const world = walkthroughWorld({ machine: platform, gate: { outcome: 'fail-after-plant' } });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, /a step failed after the plant/);
    assert.equal(world.entries[platform].size, 0);
    assert.ok(world.at(TORN_DOWN[platform]) < world.at(`${platform}:clear ai:openai`));
    assert.deepEqual(world.events('ai-keys-removed')[0].entries, { 'ai:openai': { removed: true, left: false } });
    assertNoValue(world, error);
  });

  for (const signal of ['SIGINT', 'SIGTERM']) {
    test(`${platform}: ${signal} after the plant stops the app, settles the gate, then deletes the key`, async (t) => {
      const world = walkthroughWorld({ machine: platform, gate: { outcome: 'wait-after-plant' } });
      t.after(world.dispose);

      const running = world.run();
      await world.reached;
      world.signals.emit(signal);
      const error = await stopsWithin(running);

      assert.equal(error.message, `walkthrough stopped by ${signal}`);
      assert.equal(world.entries[platform].size, 0);
      // The app stops, which ends the gate's wait; the gate settles; only then the cleanup.
      const order = [STOPPED[platform], `${platform}:gate-ended`, `${platform}:notion-cleanup`, TORN_DOWN[platform], `${platform}:clear ai:openai`];
      assert.deepEqual(order.map((line) => world.at(line)).toSorted((left, right) => left - right), order.map((line) => world.at(line)));
      assert.ok(order.every((line) => world.at(line) >= 0), JSON.stringify(world.log));
      assert.ok(!world.log.includes(`${platform}:break-config`), 'the gate unwound without changing the machine');
      assert.ok(world.log.includes('tunnel-killed'));
      assert.equal(world.events('stopped-gate-settled').length, 1);
      assert.deepEqual(world.events('ai-keys-removed')[0].entries, { 'ai:openai': { removed: true, left: false } });
      assert.equal(world.events('run-complete').length, 0);
      assert.equal(world.signals.listenerCount(signal), 0, 'the handlers go with the run');
      assertNoValue(world, error);
    });
  }

  test(`${platform}: a gate that keeps going after the signal can change nothing more`, async (t) => {
    const world = walkthroughWorld({ machine: platform, gate: { outcome: 'keep-going-after-plant' } });
    t.after(world.dispose);

    const running = world.run();
    await world.reached;
    world.signals.emit('SIGINT');
    await until(() => world.log.includes(STOPPED[platform]), 'the app to be stopped');
    assert.ok(!world.log.includes(`${platform}:notion-cleanup`), 'cleanup waits for the gate to settle');
    world.resume();
    const error = await stopsWithin(running);

    assert.equal(error.message, 'walkthrough stopped by SIGINT');
    assert.ok(!world.log.includes(`${platform}:break-config`));
    assert.ok(world.at(`${platform}:gate-ended`) < world.at(`${platform}:notion-cleanup`));
    assert.equal(world.entries[platform].size, 0);
  });

  test(`${platform}: a Notion connect that completes after the signal stores nothing the cleanup misses`, async (t) => {
    const world = walkthroughWorld({ machine: platform, gate: { outcome: 'connect' } });
    t.after(world.dispose);

    const running = world.run();
    await world.reached;
    world.signals.emit('SIGINT');
    await until(() => world.log.includes(STOPPED[platform]), 'the app to be stopped');
    // The request the app was serving ends now, after its app was stopped.
    world.connect();
    const error = await stopsWithin(running);

    assert.equal(error.message, 'walkthrough stopped by SIGINT');
    const order = [STOPPED[platform], `${platform}:connect-completed`, `${platform}:gate-ended`, `${platform}:notion-cleanup`, `${platform}:clear ai:openai`];
    assert.ok(order.every((line) => world.at(line) >= 0), JSON.stringify(world.log));
    assert.deepEqual(order.map((line) => world.at(line)).toSorted((left, right) => left - right), order.map((line) => world.at(line)));
    assert.equal(world.entries[platform].has(NOTION_ACCOUNT), false, 'no Notion token is left');
    assert.equal(world.entries[platform].size, 0);
    assertNoValue(world, error);
  });

  test(`${platform}: a signal before the plant stops the run with nothing planted or deleted`, async (t) => {
    const world = walkthroughWorld({ machine: platform, gate: { outcome: 'wait-before-plant' } });
    t.after(world.dispose);

    const running = world.run();
    await world.reached;
    world.signals.emit('SIGTERM');
    const error = await stopsWithin(running);

    assert.equal(error.message, 'walkthrough stopped by SIGTERM');
    assert.ok(!world.log.includes(`${platform}:patch-config`), 'the gate cannot plant once stopped');
    assert.ok(!world.log.includes(`${platform}:clear ai:openai`));
    assert.ok(world.log.includes(TORN_DOWN[platform]));
    assert.deepEqual(world.events('ai-keys-removed').map(({ armed }) => armed), [false]);
  });

  test(`${platform}: a signal during the final check fails the run instead of completing it`, async (t) => {
    const world = walkthroughWorld({ machine: platform, deferFinal: true });
    t.after(world.dispose);

    const running = world.run();
    await world.finalReached;
    world.signals.emit('SIGTERM');
    // The final check then answers as if all were well.
    world.finishFinal();
    const error = await stopsWithin(running);

    assert.equal(error.message, 'walkthrough stopped by SIGTERM');
    assert.equal(world.events('run-complete').length, 0);
    assert.equal(world.events('run-failed').at(-1).error, 'walkthrough stopped by SIGTERM');
    assert.deepEqual(world.printed, [], 'nothing is reported as a result');
    assert.equal(world.entries[platform].size, 0, 'the key went with the machine\'s cleanup');
  });

  test(`${platform}: failing trace writes after the plant still tear the machine down and remove the key`, async (t) => {
    const world = walkthroughWorld({ machine: platform, gate: { outcome: 'fail-after-plant', breakTrace: true } });
    t.after(world.dispose);

    const error = await failure(world.run());

    // The run's own error reaches the caller, and says the trace is incomplete.
    assert.match(error.message, /^a step failed after the plant; the trace could not be written: EISDIR/);
    assert.ok(world.log.includes(`${platform}:notion-cleanup`));
    assert.ok(world.log.includes(TORN_DOWN[platform]), 'the machine is torn down');
    assert.ok(world.at(TORN_DOWN[platform]) < world.at(`${platform}:clear ai:openai`), 'and the key removed after it');
    assert.equal(world.entries[platform].size, 0);
    assert.ok(world.log.includes('tunnel-killed'));
    if (platform === 'windows') {
      assert.deepEqual(world.windows, { configSwapped: false, locked: false, driverRunning: false }, 'Windows is restored');
    }
    assertNoValue(world, error, { trace: false });
  });

  test(`${platform}: a stop that fails after a signal still leaves the full teardown to run and restore`, async (t) => {
    // The early stop's last wait times out, though the app has gone by then.
    const world = walkthroughWorld({
      machine: platform,
      gate: { outcome: 'wait-after-plant' },
      machineFaults: { fedora: { stopAppsFails: true }, windows: { fail: { 'processes-gone': 1 } } },
    });
    t.after(world.dispose);

    const running = world.run();
    await world.reached;
    world.signals.emit('SIGINT');
    const error = await stopsWithin(running);

    assert.equal(error.message, 'walkthrough stopped by SIGINT', 'the run ends with its first error');
    assert.ok(world.at(STOPPED[platform]) >= 0 && world.at(STOPPED[platform]) < world.at(TORN_DOWN[platform]), JSON.stringify(world.log));
    assert.ok(world.at(TORN_DOWN[platform]) < world.at(`${platform}:clear ai:openai`));
    assert.match(world.events('cleanup-error')[0].error, /Fedora process survived app exit|Windows process state timed out/);
    assert.equal(world.events('stopped-gate-settled').length, 1);
    assert.equal(world.entries[platform].size, 0, 'the key is removed');
    if (platform === 'windows') {
      assert.deepEqual(world.windows, { configSwapped: false, locked: false, driverRunning: false }, 'Windows is restored');
    }
  });

  test(`${platform}: a lookup error before the run refuses it before anything changes`, async (t) => {
    const world = walkthroughWorld({ machine: platform, faults: { [platform]: { searchFailsFrom: 1, readErrorFrom: 1 } } });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, platform === 'fedora'
      ? /Secret Service search for ai:openai failed \(exit 1\): Call failed/
      : /could not check ai:openai\.io\.github\.zulucodedesign\.SecondBrain: CredReadW failed with Win32 error 1312/);
    assert.deepEqual(changes(world.log), []);
    assert.deepEqual(world.events('ai-keys-checked'), [], 'a failed lookup is not recorded as an answer');
    assert.equal(world.events('run-failed').length, 1);
  });

  test(`${platform}: a lookup error at the plant stops the run with nothing planted or deleted`, async (t) => {
    const world = walkthroughWorld({ machine: platform, faults: { [platform]: { searchFailsFrom: 2, readErrorFrom: 3 } } });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, /Secret Service search for ai:openai failed|CredReadW failed with Win32 error 1312/);
    assert.ok(!world.log.includes(`${platform}:patch-config`));
    assert.ok(!world.log.includes(`${platform}:clear ai:openai`));
    assert.deepEqual(world.events('ai-keys-armed').map(({ armed, entries }) => ({ armed, entries })), [{ armed: false, entries: undefined }]);
    assert.deepEqual(world.events('ai-keys-removed').map(({ armed }) => armed), [false]);
  });

  test(`${platform}: a delete error fails the run and records the entry as unknown`, async (t) => {
    const world = walkthroughWorld({ machine: platform, faults: { [platform]: { clearFails: true, deleteError: 5 } } });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, platform === 'fedora'
      ? /AI key cleanup: ai:openai on fedora: secret-tool clear for ai:openai failed \(exit 1\)/
      : /AI key cleanup: ai:openai on windows: could not check .*: CredDeleteW failed with Win32 error 5/);
    assert.deepEqual(world.events('ai-keys-removed')[0].entries, { 'ai:openai': { removed: false, left: null } });
    assert.equal(world.events('cleanup-error').length, 1);
    assert.equal(world.events('run-complete').length, 0);
    assertNoValue(world, error);
  });
}

test('fedora: a locked entry before the run counts as present and refuses it', async (t) => {
  const world = walkthroughWorld({ machine: 'fedora', heldLocked: ['fedora'] });
  t.after(world.dispose);

  const error = await failure(world.run());

  assert.match(error.message, /keyring already holds .*fedora \(account ai:openai/);
  assert.deepEqual(changes(world.log), []);
  assert.deepEqual(world.events('ai-keys-checked')[0].entries, { 'ai:openai': { existed: true } });
  assert.equal(world.entries.fedora.get('ai:openai'), REAL_KEY);
  assertNoValue(world, error);
});

test('fedora: a locked entry that appears before the plant stops it with nothing planted or deleted', async (t) => {
  const world = walkthroughWorld({
    machine: 'fedora',
    launch: { fedora: (entries, locked) => entries.set('ai:openai', REAL_KEY) && locked.add('ai:openai') },
  });
  t.after(world.dispose);

  const error = await failure(world.run());

  assert.match(error.message, /keyring gained an AI key entry .*fedora \(account ai:openai/);
  assert.ok(!world.log.includes('fedora:patch-config'));
  assert.ok(!world.log.includes('fedora:clear ai:openai'));
  assert.equal(world.entries.fedora.get('ai:openai'), REAL_KEY);
});

test('fedora: a planted entry that is locked by cleanup time is reported as left', async (t) => {
  const world = walkthroughWorld({ machine: 'fedora', gate: { afterPlant: (entries, locked) => locked.add('ai:openai') } });
  t.after(world.dispose);

  const error = await failure(world.run());

  assert.match(error.message, /AI key cleanup: ai:openai is still in the fedora keyring/);
  assert.deepEqual(world.events('ai-keys-removed')[0].entries, { 'ai:openai': { removed: false, left: true } });
  assert.equal(world.events('run-complete').length, 0);
  assertNoValue(world, error);
});

test('windows: an entry the script saw only on its second read counts as present, before the run and at the plant', async (t) => {
  // The first call is the check before the run; the second the re-check before the plant.
  for (const [from, refusal] of [[1, /keyring already holds .*windows \(account ai:openai/], [2, /keyring gained an AI key entry .*windows \(account ai:openai/]]) {
    const world = walkthroughWorld({ machine: 'windows', faults: { windows: { appearsBetweenReadsFrom: from } } });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, refusal);
    assert.ok(!world.log.includes('windows:patch-config'), 'nothing is planted');
    assert.ok(!world.log.includes('windows:clear ai:openai'), 'nothing is deleted');
    assert.equal(world.entries.windows.get('ai:openai'), REAL_KEY);
    const answered = world.events(from === 1 ? 'ai-keys-checked' : 'ai-keys-armed')[0];
    assert.deepEqual(answered.entries, { 'ai:openai': { existed: true } });
  }
});

test('both machines are checked before anything changes, and a refusal names each machine holding an entry', async (t) => {
  const world = walkthroughWorld({ held: ['windows'] });
  t.after(world.dispose);

  const error = await failure(world.run());

  assert.match(error.message, /windows \(account ai:openai/);
  assert.doesNotMatch(error.message, /fedora \(account/);
  assert.deepEqual(world.log, ['commits', 'deploy-tools', 'fedora:lookup ai:openai', 'windows:lookup ai:openai']);
  assert.deepEqual(world.events('ai-keys-checked').map(({ machine, entries }) => [machine, entries['ai:openai'].existed]), [
    ['fedora', false], ['windows', true],
  ]);
  assert.equal(world.entries.windows.get('ai:openai'), REAL_KEY);
});

test('a run of both machines plants and removes the key on each, one machine after the other', async (t) => {
  const world = walkthroughWorld();
  t.after(world.dispose);

  await world.run();

  assert.equal(world.entries.fedora.size + world.entries.windows.size, 0);
  assert.ok(world.at('fedora:clear ai:openai') < world.at('windows:install'));
  assert.deepEqual(world.events('ai-keys-removed').map(({ machine, armed }) => [machine, armed]), [['fedora', true], ['windows', true]]);
});

test('the walkthrough may plant only the AI keys it checked', async (t) => {
  const world = walkthroughWorld({ machine: 'fedora', gate: { patch: { ai_api_key: PLANTED_KEY } } });
  t.after(world.dispose);

  const error = await failure(world.run());

  assert.match(error.message, /may plant only the AI keys it checked, not ai_api_key/);
  assert.ok(!world.log.includes('fedora:patch-config'));
  assert.deepEqual(world.events('ai-keys-removed').map(({ armed }) => armed), [false]);
  assertNoValue(world, error);
});

// The machine teardown every gate ends with (finishMachine, then the Windows machine's cleanup and
// finalizeWindows), run for real over fakeWindowsIo, with a trace the test can make unwritable.
async function windowsTeardown(t, faults = {}) {
  const root = mkdtempSync(join(tmpdir(), 'alpha-teardown-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const tracePath = join(root, 'trace.jsonl');
  const gone = deferred();
  const world = {
    log: [],
    windows: { configSwapped: false, locked: false, driverRunning: false },
    apps: { windows: { alive: false, gone: gone.promise, stopped: gone.settle } },
    tracePath,
    // A full evidence disk, as far as appendFileSync can tell.
    breakTrace: () => mkdirSync(tracePath, { recursive: true }),
    events: () => (existsSync(tracePath) ? readFileSync(tracePath, 'utf8').trim().split('\n').filter(Boolean).map((line) => JSON.parse(line)) : []),
  };
  world.machine = fakeWindowsMachine(world, 'fake-run', faults);
  world.machine.prepare();
  await world.machine.startDriver();
  world.log.length = 0;
  return world;
}

const RESTORED = { configSwapped: false, locked: false, driverRunning: false };

test('windows teardown: every trace write failing still stops the driver and restores the config', async (t) => {
  const world = await windowsTeardown(t);
  world.breakTrace();

  const error = await finishMachine(world.machine, world.tracePath);

  assert.deepEqual(world.log, ['windows:tunnel-killed', 'windows:stop-driver', 'windows:processes-gone 60000', 'windows:finalize']);
  assert.deepEqual(world.windows, RESTORED);
  assert.match(error.message, /^the trace could not be written: EISDIR/);
});

test('windows teardown: a failed driver stop still stops the app and restores the config, trace or not', async (t) => {
  for (const broken of [false, true]) {
    const world = await windowsTeardown(t, { fail: { 'stop-driver': 1 } });
    if (broken) world.breakTrace();

    const error = await finishMachine(world.machine, world.tracePath);

    assert.deepEqual(world.log, [
      'windows:tunnel-killed', 'windows:stop-driver', 'windows:processes-gone 60000',
      'windows:cleanup-processes', 'windows:processes-gone 15000', 'windows:finalize',
    ]);
    assert.equal(world.windows.configSwapped, false, 'the config is restored');
    assert.equal(world.windows.locked, false);
    // The driver stop's error is the first real error, and the trace failure is named after it.
    assert.match(error.message, broken
      ? /^ssh exited 255: the stop-driver action could not reach Windows; the trace could not be written: EISDIR/
      : /^ssh exited 255: the stop-driver action could not reach Windows$/);
    if (!broken) {
      assert.deepEqual(world.events().map(({ event, machine }) => `${event} ${machine}`), [
        'cleanup-error windows-driver', 'forced-process-cleanup windows', 'process-final windows', 'config-restored windows', 'cleanup-error windows',
      ]);
    }
  }
});

test('windows teardown: a failed forced stop still waits again and attempts the restore, which waits for the app', async (t) => {
  const world = await windowsTeardown(t, { appSurvivesDriverStop: true, fail: { 'cleanup-processes': 1 } });

  const error = await finishMachine(world.machine, world.tracePath);

  assert.deepEqual(world.log, [
    'windows:tunnel-killed', 'windows:stop-driver', 'windows:processes-gone 60000',
    'windows:cleanup-processes', 'windows:processes-gone 15000', 'windows:finalize',
  ]);
  // The worker refuses to restore while the app runs, so the config stays swapped and the run fails.
  assert.equal(world.windows.configSwapped, true);
  assert.match(error.message, /^Windows process state timed out/);
  assert.deepEqual(world.events().filter(({ event }) => event === 'cleanup-error').map(({ machine, error: message }) => [machine, message.split(':')[0]]), [
    ['windows-processes', 'ssh exited 255'], ['windows-processes', 'Windows process state timed out'],
    ['windows-config', 'refusing to restore config while installed processes remain'], ['windows', 'Windows process state timed out'],
  ]);
});

test('windows teardown: an early stop goes on past a failed driver stop and trace write, and cleanup stops the driver again', async (t) => {
  const world = await windowsTeardown(t, { fail: { 'stop-driver': 1 } });
  world.breakTrace();

  await assert.rejects(world.machine.stopApps(world.tracePath), /^Error: ssh exited 255: the stop-driver action could not reach Windows; the trace could not be written: EISDIR/);
  assert.deepEqual(world.log, ['windows:tunnel-killed', 'windows:stop-driver', 'windows:cleanup-processes', 'windows:processes-gone 15000']);
  assert.equal(world.apps.windows.alive, false, 'the app is stopped');

  world.log.length = 0;
  const error = await finishMachine(world.machine, world.tracePath);

  assert.deepEqual(world.log, ['windows:tunnel-killed', 'windows:stop-driver', 'windows:processes-gone 60000', 'windows:finalize']);
  assert.deepEqual(world.windows, RESTORED);
  assert.match(error.message, /^the trace could not be written/);
});

test('teardown: finishMachine names a trace it could not write even when the teardown itself passed', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'alpha-teardown-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const tracePath = join(root, 'trace.jsonl');
  mkdirSync(tracePath);
  // Fedora's teardown writes nothing itself and returns its final process report.
  const machine = { name: 'fedora', cleanup: async () => ({ processes: [] }) };

  const error = await finishMachine(machine, tracePath);

  assert.match(error.message, /^the trace could not be written: EISDIR/);
  assert.equal(error.traceError.code, 'EISDIR');
});

for (const platform of PLATFORMS) {
  test(`${platform}: a passing gate whose trace breaks after the plant fails the run, torn down and with no key left`, async (t) => {
    const world = walkthroughWorld({ machine: platform, gate: { breakTrace: true } });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, /^the trace could not be written: EISDIR/);
    assert.ok(world.at(TORN_DOWN[platform]) < world.at(`${platform}:clear ai:openai`));
    assert.equal(world.entries[platform].size, 0);
    assert.ok(!world.log.includes('windows:uninstall'), 'a run that failed keeps the candidate installed');
    assert.deepEqual(world.printed, []);
    if (platform === 'windows') assert.deepEqual(world.windows, { configSwapped: false, locked: false, driverRunning: false });
  });
}

test('teardown: finishMachine reports a passing teardown as passed, and never throws', async (t) => {
  const world = await windowsTeardown(t);

  assert.equal(await finishMachine(world.machine, world.tracePath), undefined);
  assert.deepEqual(world.windows, RESTORED);
  assert.deepEqual(world.events().map(({ event }) => event), ['driver-stopped', 'process-final', 'config-restored']);
  assert.equal(await finishMachine(undefined, world.tracePath), undefined, 'a machine never made has nothing to tear down');
});

test('fedora teardown: every exact PID is signalled even when one before it fails', () => {
  const signalled = [];
  const kill = (pid, signal) => {
    signalled.push(`${pid} ${signal}`);
    if (pid === 2) throw Object.assign(new Error('kill EPERM'), { code: 'EPERM' });
  };

  assert.throws(() => killEach([{ pid: 1 }, { pid: 2 }, { pid: 3 }], 'SIGKILL', kill), /EPERM/);
  assert.deepEqual(signalled, ['1 SIGKILL', '2 SIGKILL', '3 SIGKILL']);
  assert.doesNotThrow(() => killEach([], 'SIGTERM', kill));
});

test('the walkthrough plants exactly the keys it checks for', () => {
  assert.deepEqual(plantedAiKeyPatch(PLANTED_KEY), { openai_api_key: PLANTED_KEY });
  assert.deepEqual(PLANTED_AI_KEYS, { openai_api_key: 'ai:openai' });
  for (const account of Object.values(PLANTED_AI_KEYS)) assert.ok(AI_KEY_ACCOUNTS.includes(account));
});

test('fedora: a Secret Service search that failed or answered oddly is never read as absent', () => {
  const store = (status, stdout, stderr = '', error) => fedoraAiKeyStore({ spawn: () => ({ status, stdout, stderr, error }) });
  const reply = (data, type = 'aoao') => JSON.stringify({ type, data });

  assert.throws(() => store(1, '', 'Call failed: Access denied').stored('ai:openai'), /search for ai:openai failed \(exit 1\): Call failed: Access denied/);
  assert.throws(() => store(null, null, '', new Error('spawn busctl ENOENT')).stored('ai:openai'), /ENOENT/);
  for (const odd of ['', 'not json', reply([[]]), reply([[], [], []]), reply([['/a'], [7]]), reply([[], []], 'ao'), JSON.stringify({ data: [[], []] })]) {
    assert.throws(() => store(0, odd).stored('ai:openai'), /did not answer with lists of unlocked and locked items/, odd);
  }
  assert.equal(store(0, reply([[], []])).stored('ai:openai'), false);
  assert.equal(store(0, reply([['/org/freedesktop/secrets/collection/login/3'], []])).stored('ai:openai'), true);
  assert.equal(store(0, reply([[], ['/org/freedesktop/secrets/collection/login/3']])).stored('ai:openai'), true, 'a locked entry is present');
});

test('the keyring stores refuse accounts that are not the app\'s AI keys', () => {
  const log = [];
  const stores = [fakeStore('fedora', new Map(), new Set(), log), fakeStore('windows', new Map(), new Set(), log)];
  for (const store of stores) {
    for (const account of ['integration:notion:vault-1', 'ai:other', 'sync:webdav:v', '', 'AI:OPENAI']) {
      assert.throws(() => store.stored(account), /not an AI key account/);
      assert.throws(() => store.remove(account), /not an AI key account/);
    }
  }
  assert.deepEqual(log, [], 'a refused account never reaches the keyring');
});

test('windows: each credential call has a result file of its own, and none is left behind', () => {
  const entries = new Map([['ai:openai', REAL_KEY]]);
  const desktop = fakeDesktopScript(entries, []);

  assert.deepEqual(windowsAiKeyAction({ account: 'ai:openai', remove: false }, desktop), { found: true, left: true });
  assert.deepEqual(windowsAiKeyAction({ account: 'ai:openai', remove: true }, desktop), { found: true, left: false });

  assert.equal(new Set(desktop.paths).size, 2);
  for (const path of desktop.paths) assert.match(path, /^D:\\SecondBrainTest\\sb88\\tools\\credential-[0-9a-f-]{36}\.json$/);
  assert.equal(desktop.files.size, 0);
});

test('windows: a result an earlier call wrote late cannot answer a later call', () => {
  const desktop = fakeDesktopScript(new Map(), []);
  let late;
  const timedOut = {
    ...desktop,
    runDesktop(scriptArguments) {
      late = /-OutputPath "([^"]+)"/.exec(scriptArguments)[1];
      throw new Error('powershell.exe exited 1: desktop script SecondBrainAlphaHarnessDesktop did not finish within 5 minutes and was stopped');
    },
  };

  assert.throws(() => windowsAiKeyAction({ account: 'ai:openai', remove: false }, timedOut), /could not check .*did not finish within 5 minutes/);
  desktop.files.set(late, JSON.stringify({ found: true, left: true }));

  assert.deepEqual(windowsAiKeyAction({ account: 'ai:openai', remove: false }, desktop), { found: false, left: false });
});

test('windows: only a well-formed Win32 error from the script reaches the error message', () => {
  const failing = (written) => ({
    tools: TOOLS,
    runDesktop() {
      throw new Error('desktop script failed with 1');
    },
    readResult: () => written,
    discard() {},
  });
  const check = (written) => () => desktopCredentialAction({ target: windowsAiKeyTarget('ai:openai') }, failing(written));

  assert.throws(check({ error: 'CredReadW failed with Win32 error 1312' }), /: CredReadW failed with Win32 error 1312$/);
  for (const written of [{ error: `CredReadW failed with Win32 error 5 ${REAL_KEY}` }, { error: REAL_KEY }, { found: true }, null]) {
    assert.throws(check(written), (error) => /: desktop script failed with 1$/.test(error.message) && !error.message.includes(REAL_KEY));
  }
});

test('windows: the worker passes on the two flags and nothing the script wrote beside them', () => {
  const desktop = fakeDesktopScript(new Map([['ai:openai', REAL_KEY]]), [], { extra: { value: REAL_KEY, length: REAL_KEY.length } });

  const answer = windowsAiKeyAction({ account: 'ai:openai', remove: false }, desktop);

  assert.deepEqual(answer, { found: true, left: true });
  assert.ok(!JSON.stringify(answer).includes(REAL_KEY));
});

test('windows: the desktop script call waits as long as alpha-harness.ps1 waits for its task', () => {
  const calls = [];
  const ran = runDesktopScript(TOOLS, '-Mode NotionToken -OutputPath "x"', (tools, args, timeout) => {
    calls.push({ tools, args, timeout });
    return { result: 0 };
  });

  assert.deepEqual(ran, { result: 0 });
  assert.equal(calls[0].timeout, DESKTOP_SCRIPT_TIMEOUT_MS);
  assert.deepEqual(calls[0].args.slice(0, 2), ['-Action', 'DesktopScript']);
  assert.throws(() => runDesktopScript(TOOLS, '-Mode X', () => ({ result: 1 })), /desktop script failed with 1/);
});

test('windows: each timeout around a desktop task is longer than the one inside it', () => {
  const script = readFileSync(new URL('../scripts/windows/alpha-harness.ps1', import.meta.url), 'utf8');
  const action = script.slice(script.indexOf("if ($Action -eq 'DesktopScript')"), script.indexOf("if ($Action -eq 'Metadata')"));

  // The waits the action makes: a stale task's stop, its own task, and that task's stop.
  assert.match(action, /\$taskDeadline = \(Get-Date\)\.AddMinutes\(5\)/);
  assert.match(action, /\$stopDeadline = \(Get-Date\)\.AddSeconds\(30\)/);
  assert.match(action, /-ExecutionTimeLimit \(New-TimeSpan -Minutes 5\)/);
  assert.equal(DESKTOP_TASK_BUDGET_MS, 30_000 + 5 * 60_000 + 30_000);
  assert.ok(DESKTOP_TASK_BUDGET_MS < DESKTOP_SCRIPT_TIMEOUT_MS && DESKTOP_SCRIPT_TIMEOUT_MS < DESKTOP_ACTION_TIMEOUT_MS);

  // A stale task is stopped before the new one is registered, and an overrunning one is stopped
  // before the action gives up on it; the task is removed either way.
  const at = (text) => action.indexOf(text);
  assert.ok(at('$staleTask.State -eq \'Running\') { Stop-DesktopTask }') > 0);
  assert.ok(at('Stop-DesktopTask }') < at('Register-ScheduledTask'));
  assert.ok(at('if ($taskState -eq \'Running\') {') < at('Stop-DesktopTask\n'), 'the overrun branch stops the task');
  assert.ok(at('Stop-DesktopTask\n') < at('did not finish within 5 minutes and was stopped'));
  assert.ok(at('} finally {') < at('Unregister-ScheduledTask'));
});

test('alpha-desktop.ps1 checks exactly the app\'s AI targets and stops on any error but ERROR_NOT_FOUND', () => {
  const script = readFileSync(new URL('../scripts/windows/alpha-desktop.ps1', import.meta.url), 'utf8');

  assert.equal(/\$appService = '([^']+)'/.exec(script)?.[1], SERVICE);
  const accounts = /\$aiAccounts = @\(([^)]*)\)/.exec(script)?.[1].split(',').map((account) => account.trim().replaceAll("'", ''));
  assert.deepEqual(accounts, AI_KEY_ACCOUNTS);
  assert.match(script, /if \(-not \$isNotionToken -and \$aiTargets -cnotcontains \$CredentialTarget\) \{\s*throw /);
  assert.equal(windowsAiKeyTarget('ai:openai'), `ai:openai.${SERVICE}`);

  // Only 0 and ERROR_NOT_FOUND pass; anything else writes the call and code alone, and exits 1.
  assert.match(script, /\$notFoundCode = 1168/);
  assert.match(script, /if \(\$statusCode -eq 0 -or \$statusCode -eq \$notFoundCode\) \{ return \}/);
  assert.match(script, /\[pscustomobject\]@\{ error = "\$callName failed with Win32 error \$statusCode" \}[^\n]*\n[^\n]*Set-Content[^\n]*\n\s*exit 1/);
  assert.match(script, /\$readStatus = \[AlphaHarness\.Credentials\]::Read\(\$CredentialTarget\)\s*\n\s*Assert-CredentialCall 'CredReadW' \$readStatus/);
  assert.match(script, /Assert-CredentialCall 'CredDeleteW' \(\[AlphaHarness\.Credentials\]::Delete\(\$CredentialTarget\)\)/);
  assert.match(script, /\$leftStatus = \[AlphaHarness\.Credentials\]::Read\(\$CredentialTarget\)\s*\n\s*Assert-CredentialCall 'CredReadW' \$leftStatus/);
  assert.match(script, /\[pscustomobject\]@\{ found = \$credentialFound; left = \$credentialLeft \}/);
});
