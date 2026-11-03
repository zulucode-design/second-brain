// The Gate 3 walkthrough plants a synthetic AI key that the app moves into the OS keyring (#252).
// It must refuse when the keyring already holds the entry, before it changes either machine, and
// remove what it planted after a pass, a failure or a signal, and only that. These tests run the
// walkthrough controller's own sequence (runWalkthroughLocked) against fake machines, fake signals,
// and fakes of `secret-tool` (Fedora) and of the Windows desktop-session script. None touches a
// real keyring, machine or process.
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const {
  AI_KEY_ACCOUNTS, DESKTOP_ACTION_TIMEOUT_MS, DESKTOP_SCRIPT_TIMEOUT_MS, DESKTOP_TASK_BUDGET_MS, PLANTED_AI_KEYS,
  desktopCredentialAction, fedoraAiKeyStore, plantedAiKeyPatch, runDesktopScript, runWalkthroughLocked,
  windowsAiKeyAction, windowsAiKeyStore, windowsAiKeyTarget,
} = await import(new URL('../scripts/alpha-harness.mjs', import.meta.url));

const SERVICE = 'io.github.zulucodedesign.SecondBrain';
const TOOLS = 'D:\\SecondBrainTest\\sb88\\tools';
// Stand-ins for a user's real key and for the key the walkthrough plants. Neither may appear in
// anything the harness records, prints or throws.
const REAL_KEY = 'sk-real-user-key-0f3a9c11-do-not-read';
const PLANTED_KEY = 'sk-alpha-harness-5b2e7d90-planted';
const PLATFORMS = ['fedora', 'windows'];

// A `secret-tool` that behaves as libsecret's does: `lookup` prints the value to stdout, which the
// operating system discards when the caller set stdio `ignore`, and exits 1 with no message when
// nothing matches; `clear` exits 1 with no message when nothing matched. `lookupFailsFrom` makes
// the nth lookup and later ones fail as a missing D-Bus session does; `clearFails` the clear.
function fakeSecretTool(entries, log, { lookupFailsFrom, clearFails = false } = {}) {
  let lookups = 0;
  return (command, args, options) => {
    assert.equal(command, 'secret-tool');
    assert.deepEqual(options.stdio, ['ignore', 'ignore', 'pipe'], 'the value goes to the null device');
    const [verb, ...pairs] = args;
    assert.deepEqual(pairs.slice(0, 3), ['service', SERVICE, 'username']);
    const account = pairs[3];
    log.push(`fedora:${verb} ${account}`);
    if (verb === 'lookup') {
      lookups += 1;
      if (lookupFailsFrom && lookups >= lookupFailsFrom) return { status: 1, stdout: null, stderr: 'secret-tool: Cannot autolaunch D-Bus without X11 $DISPLAY' };
      return { status: entries.has(account) ? 0 : 1, stdout: null, stderr: '' };
    }
    assert.equal(verb, 'clear');
    if (clearFails) return { status: 1, stdout: null, stderr: 'secret-tool: Cannot create an item in a locked collection' };
    if (!entries.delete(account)) return { status: 1, stdout: null, stderr: '' };
    return { status: 0, stdout: null, stderr: '' };
  };
}

// alpha-desktop.ps1 in NotionToken mode, run as runDesktopScript runs it: the same target guard;
// found/left written to the result file; and for a Win32 error other than ERROR_NOT_FOUND, the
// call and its code written instead and a failed task, which runDesktopScript turns into a throw.
// `extra` models a script that writes more than it should.
function fakeDesktopScript(entries, log, { readErrorFrom, deleteError, extra = {} } = {}) {
  const files = new Map();
  const aiTargets = AI_KEY_ACCOUNTS.map((account) => `${account}.${SERVICE}`);
  const paths = [];
  let reads = 0;
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
function fakeStore(platform, entries, log, faults) {
  if (platform === 'fedora') return fedoraAiKeyStore({ spawn: fakeSecretTool(entries, log, faults) });
  const desktop = fakeDesktopScript(entries, log, faults);
  const viaJson = (value) => JSON.parse(JSON.stringify(value));
  return windowsAiKeyStore((request) => viaJson(windowsAiKeyAction(viaJson(request), desktop)));
}

// A machine as linuxDriverMachine and windowsDriverMachine present it to the walkthrough, logging
// each call. `onLaunchEntry` models what the Start-menu launch may do to the keyring.
function fakeMachine(name, log, { onLaunchEntry } = {}) {
  const machine = {
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
    async cleanup() {
      log.push(`${name}:cleanup`);
    },
  };
  if (name === 'windows') {
    machine.install = () => {
      log.push('windows:install');
      return {};
    };
  }
  return machine;
}

// The walkthrough gate as walkthroughGate drives the machine up to and past the plant, ending as
// `outcome` says. After the plant, the app's next launch files the key in the keyring.
function fakeGate(entries, log, { outcome = 'pass', patch = plantedAiKeyPatch(PLANTED_KEY), reached, resume } = {}) {
  return async (machine) => {
    machine.session();
    await machine.launchEntry();
    await machine.startDriver();
    if (outcome === 'fail-before-plant') throw new Error('the WebDriver session did not start');
    if (outcome === 'wait-before-plant') {
      reached();
      await resume;
    }
    machine.patchConfig(patch);
    entries[machine.name].set('ai:openai', PLANTED_KEY);
    log.push(`${machine.name}:app-filed-key`);
    if (outcome === 'fail-after-plant') throw new Error('a step failed after the plant');
    if (outcome === 'wait-after-plant') {
      reached();
      await resume;
      machine.breakConfig();
    }
  };
}

/**
 * One controller run's world. `machine` is the --machine option. `held` puts a real key in a
 * platform's keyring before the run; `faults` and `launch` are per platform.
 */
function walkthroughWorld({ machine, held = [], faults = {}, launch = {}, gate = {} } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'alpha-keyring-'));
  const log = [];
  const printed = [];
  const entries = { fedora: new Map(), windows: new Map() };
  for (const platform of held) entries[platform].set('ai:openai', REAL_KEY);
  let reachedGate;
  const reached = new Promise((resolveReached) => {
    reachedGate = resolveReached;
  });
  let resumeGate;
  const resume = new Promise((resolveResume) => {
    resumeGate = resolveResume;
  });
  const signals = new EventEmitter();
  const io = {
    signals,
    notion: () => ({ token: 'fake-notion-token', page: 'Fake page' }),
    commits() {
      log.push('commits');
      return { candidateCommit: 'c'.repeat(40), harnessCommit: 'h'.repeat(40) };
    },
    deployTools: () => log.push('deploy-tools'),
    evidencePath: (runId) => join(root, 'trace', `${runId}.jsonl`),
    keyStore: (name) => fakeStore(name, entries[name], log, faults[name]),
    installCandidate: () => log.push('install-candidate'),
    prepareRun(early) {
      log.push('prepare-run');
      return { ...early, fedoraPackage: {}, recovered: { recovered: false } };
    },
    ollamaTunnel: () => ({ exitCode: null, kill: () => log.push('tunnel-killed') }),
    ollamaModels: async (label) => {
      log.push(`ollama-${label}`);
      return {};
    },
    makeMachine: (name) => fakeMachine(name, log, { onLaunchEntry: launch[name] && (() => launch[name](entries[name])) }),
    gate: fakeGate(entries, log, { ...gate, reached: reachedGate, resume }),
    notionCleanup: async (target) => {
      log.push(`${target.name}:notion-cleanup`);
      return {};
    },
    sleep: async () => {},
    print: (line) => printed.push(line),
  };
  const options = { linuxRoot: root, holdMinutes: 70, windowsInstaller: 'D:\\SecondBrainTest\\setup.exe', machine };
  return {
    entries, log, printed, signals, reached, resume: resumeGate,
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
    dispose: () => rmSync(root, { recursive: true, force: true }),
  };
}

// Everything a run could print or keep: the trace, what it printed, and its error.
function assertNoValue(world, error) {
  const text = `${world.traceText()}\n${world.printed.join('\n')}\n${error?.message ?? ''}`;
  for (const value of [REAL_KEY, PLANTED_KEY]) assert.ok(!text.includes(value), 'no key value is recorded, printed or thrown');
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

const changes = (log) => log.filter((line) => /install|prepare|launch|driver|patch|break|cleanup|clear/.test(line));

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
    const at = (line) => world.log.indexOf(line);
    assert.ok(at(`${platform}:lookup ai:openai`) < at('install-candidate'), 'the check comes before the install');
    assert.ok(at('install-candidate') < at('prepare-run'));
    // The re-check comes right before the patch, and the removal after the app and driver stopped.
    assert.equal(world.log[at(`${platform}:patch-config`) - 1], `${platform}:lookup ai:openai`);
    assert.ok(at(`${platform}:cleanup`) < at(`${platform}:clear ai:openai`));
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
    assert.ok(world.log.includes(`${platform}:cleanup`), 'the machine is still cleaned up');
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
    assert.ok(world.log.includes(`${platform}:cleanup`));
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
    assert.ok(world.log.indexOf(`${platform}:cleanup`) < world.log.indexOf(`${platform}:clear ai:openai`));
    assert.deepEqual(world.events('ai-keys-removed')[0].entries, { 'ai:openai': { removed: true, left: false } });
    assertNoValue(world, error);
  });

  for (const signal of ['SIGINT', 'SIGTERM']) {
    test(`${platform}: ${signal} after the plant stops the run, and the same cleanup deletes the key`, async (t) => {
      const world = walkthroughWorld({ machine: platform, gate: { outcome: 'wait-after-plant' } });
      t.after(world.dispose);

      const running = world.run();
      await world.reached;
      world.signals.emit(signal);
      const error = await stopsWithin(running);

      assert.equal(error.message, `walkthrough stopped by ${signal}`);
      assert.equal(world.entries[platform].size, 0);
      const at = (line) => world.log.indexOf(line);
      assert.ok(at(`${platform}:notion-cleanup`) >= 0 && at(`${platform}:cleanup`) >= 0);
      assert.ok(at(`${platform}:cleanup`) < at(`${platform}:clear ai:openai`));
      assert.ok(world.log.includes('tunnel-killed'));
      assert.deepEqual(world.events('ai-keys-removed')[0].entries, { 'ai:openai': { removed: true, left: false } });
      assert.equal(world.signals.listenerCount(signal), 0, 'the handlers go with the run');

      // The stopped walkthrough can change nothing more.
      world.resume();
      await new Promise((resolveTurn) => setImmediate(resolveTurn));
      assert.ok(!world.log.includes(`${platform}:break-config`));
      assertNoValue(world, error);
    });
  }

  test(`${platform}: a signal before the plant stops the run with nothing planted or deleted`, async (t) => {
    const world = walkthroughWorld({ machine: platform, gate: { outcome: 'wait-before-plant' } });
    t.after(world.dispose);

    const running = world.run();
    await world.reached;
    world.signals.emit('SIGTERM');
    const error = await stopsWithin(running);
    world.resume();
    await new Promise((resolveTurn) => setImmediate(resolveTurn));

    assert.equal(error.message, 'walkthrough stopped by SIGTERM');
    assert.ok(!world.log.includes(`${platform}:patch-config`), 'the gate cannot plant once stopped');
    assert.ok(!world.log.includes(`${platform}:clear ai:openai`));
    assert.ok(world.log.includes(`${platform}:cleanup`));
    assert.deepEqual(world.events('ai-keys-removed').map(({ armed }) => armed), [false]);
  });

  test(`${platform}: a lookup error before the run refuses it before anything changes`, async (t) => {
    const world = walkthroughWorld({ machine: platform, faults: { [platform]: { lookupFailsFrom: 1, readErrorFrom: 1 } } });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, platform === 'fedora'
      ? /secret-tool lookup for ai:openai failed \(exit 1\): .*D-Bus/
      : /could not check ai:openai\.io\.github\.zulucodedesign\.SecondBrain: CredReadW failed with Win32 error 1312/);
    assert.deepEqual(changes(world.log), []);
    assert.deepEqual(world.events('ai-keys-checked'), [], 'a failed lookup is not recorded as an answer');
    assert.equal(world.events('run-failed').length, 1);
  });

  test(`${platform}: a lookup error at the plant stops the run with nothing planted or deleted`, async (t) => {
    const world = walkthroughWorld({ machine: platform, faults: { [platform]: { lookupFailsFrom: 2, readErrorFrom: 3 } } });
    t.after(world.dispose);

    const error = await failure(world.run());

    assert.match(error.message, /secret-tool lookup for ai:openai failed|CredReadW failed with Win32 error 1312/);
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
  assert.ok(world.log.indexOf('fedora:clear ai:openai') < world.log.indexOf('windows:install'));
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

test('the walkthrough plants exactly the keys it checks for', () => {
  assert.deepEqual(plantedAiKeyPatch(PLANTED_KEY), { openai_api_key: PLANTED_KEY });
  assert.deepEqual(PLANTED_AI_KEYS, { openai_api_key: 'ai:openai' });
  for (const account of Object.values(PLANTED_AI_KEYS)) assert.ok(AI_KEY_ACCOUNTS.includes(account));
});

test('fedora: a secret-tool that failed is never read as an absent entry', () => {
  const store = (status, stderr, error) => fedoraAiKeyStore({ spawn: () => ({ status, stdout: null, stderr, error }) });

  assert.throws(() => store(1, 'secret-tool: Cannot autolaunch D-Bus without X11 $DISPLAY').stored('ai:openai'), /failed \(exit 1\).*D-Bus/);
  assert.throws(() => store(127, '').stored('ai:openai'), /failed \(exit 127\)/);
  assert.throws(() => store(null, '', new Error('spawn secret-tool ENOENT')).stored('ai:openai'), /ENOENT/);
  assert.equal(store(1, '').stored('ai:openai'), false);
});

test('the keyring stores refuse accounts that are not the app\'s AI keys', () => {
  const log = [];
  const stores = [fakeStore('fedora', new Map(), log), fakeStore('windows', new Map(), log)];
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
