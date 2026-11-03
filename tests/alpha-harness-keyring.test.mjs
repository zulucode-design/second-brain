// The Gate 3 walkthrough plants a synthetic AI key that the app moves into the OS keyring (#252).
// It must refuse when the keyring already holds the entry, and remove what it planted. Every test
// here runs against fakes of `secret-tool` (Fedora) and of the Windows desktop-session script. None
// touches a real keyring, machine or process.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const {
  AI_KEY_ACCOUNTS, PLANTED_AI_KEYS, checkNoAiKeys, fedoraAiKeyStore, plantedAiKeyPatch, removePlantedAiKeys,
  windowsAiKeyAction, windowsAiKeyStore, windowsAiKeyTarget,
} = await import(new URL('../scripts/alpha-harness.mjs', import.meta.url));

const SERVICE = 'io.github.zulucodedesign.SecondBrain';
const TOOLS = 'D:\\SecondBrainTest\\sb88\\tools';
// Stand-ins for a user's real key and for the key the walkthrough plants. Neither may appear in
// anything the harness records, prints, throws or passes to a process.
const REAL_KEY = 'sk-real-user-key-0f3a9c11-do-not-read';
const PLANTED_KEY = 'sk-alpha-harness-5b2e7d90-planted';
const VALUES = [REAL_KEY, PLANTED_KEY];

// What both operating systems hold: account -> value, plus every call made against it.
function fakeKeyring(initial = {}) {
  return { entries: new Map(Object.entries(initial)), calls: [] };
}

// A `secret-tool` that behaves as libsecret's does: `lookup` prints the value to stdout, which the
// operating system discards when the caller set stdio `ignore`, and exits 1 with no message when
// nothing matches. `clear` exits 1 with no message when nothing matched.
function fakeSecretTool(keyring, { clears = true } = {}) {
  return (command, args, options) => {
    assert.equal(command, 'secret-tool');
    const [verb, ...pairs] = args;
    const attributes = {};
    for (let index = 0; index < pairs.length; index += 2) attributes[pairs[index]] = pairs[index + 1];
    assert.equal(attributes.service, SERVICE);
    assert.deepEqual(Object.keys(attributes).sort(), ['service', 'username']);
    keyring.calls.push({ platform: 'fedora', verb, account: attributes.username, stdio: options.stdio });
    const value = keyring.entries.get(attributes.username);
    if (verb === 'lookup') {
      if (value === undefined) return { status: 1, stdout: null, stderr: '' };
      return { status: 0, stdout: options.stdio?.[1] === 'ignore' ? null : value, stderr: '' };
    }
    assert.equal(verb, 'clear');
    if (value === undefined) return { status: 1, stdout: null, stderr: '' };
    if (clears) keyring.entries.delete(attributes.username);
    return { status: 0, stdout: null, stderr: '' };
  };
}

// alpha-desktop.ps1 in NotionToken mode, as the scheduled task runs it: the same guard on the
// credential target, and a JSON file holding `found` and `left`. A buggy or hostile script is
// modelled by `extra`, fields the harness must not pass on.
function fakeDesktopScript(keyring, { extra = {}, clears = true } = {}) {
  const files = new Map();
  const aiTargets = AI_KEY_ACCOUNTS.map((account) => `${account}.${SERVICE}`);
  return {
    files,
    runDesktop(scriptArguments) {
      assert.match(scriptArguments, /^-Mode NotionToken -OutputPath "[^"]+" -CredentialTarget "[^"]+"( -RemoveCredential)?$/);
      const outputPath = /-OutputPath "([^"]+)"/.exec(scriptArguments)[1];
      const target = /-CredentialTarget "([^"]+)"/.exec(scriptArguments)[1];
      const remove = scriptArguments.endsWith(' -RemoveCredential');
      assert.ok(outputPath.startsWith('D:\\SecondBrainTest\\'), 'the script only writes under D:\\SecondBrainTest');
      if (!target.startsWith('integration:notion:') && !aiTargets.includes(target)) {
        throw new Error(`not a credential the harness manages: ${target}`);
      }
      const account = target.slice(0, -(SERVICE.length + 1));
      keyring.calls.push({ platform: 'windows', verb: remove ? 'clear' : 'lookup', account });
      const found = keyring.entries.has(account);
      if (found && remove && clears) keyring.entries.delete(account);
      files.set(outputPath, JSON.stringify({ found, left: keyring.entries.has(account), ...extra }));
    },
    readResult: (path) => JSON.parse(files.get(path)),
    discard: (path) => { files.delete(path); },
  };
}

// The two code paths, each over the same fake keyring.
const PLATFORMS = [
  {
    name: 'fedora',
    store: (keyring, options) => fedoraAiKeyStore({ spawn: fakeSecretTool(keyring, options) }),
  },
  {
    name: 'windows',
    store(keyring, options = {}) {
      const desktop = fakeDesktopScript(keyring, options);
      const store = windowsAiKeyStore((request) => windowsAiKeyAction(request, { tools: TOOLS, ...desktop }));
      return Object.assign(store, { files: desktop.files });
    },
  },
];

function trace() {
  const events = [];
  return { events, record: (machine, event, value) => events.push({ machine, event, ...value }) };
}

// Everything a run could print or keep: the trace, errors, and what was handed to a process.
function assertNoValue(label, ...captured) {
  const text = JSON.stringify(captured);
  for (const value of VALUES) assert.ok(!text.includes(value), `${label} must not hold a key value`);
}

for (const platform of PLATFORMS) {
  test(`${platform.name}: an existing entry stops the run before it changes anything`, () => {
    const keyring = fakeKeyring({ 'ai:openai': REAL_KEY });
    const store = platform.store(keyring);
    const { events, record } = trace();

    let refusal;
    assert.throws(() => checkNoAiKeys([store], record), (error) => { refusal = error; return true; });

    assert.match(refusal.message, new RegExp(`${platform.name} \\(account ai:openai, service ${SERVICE.replaceAll('.', '\\.')}\\)`));
    assert.match(refusal.message, /by hand/);
    assert.deepEqual(events, [{
      machine: platform.name, event: 'ai-keys-checked', service: SERVICE, entries: { 'ai:openai': { existed: true } },
    }]);
    // The check read and nothing else: the entry is as it was, and nothing was cleared.
    assert.equal(keyring.entries.get('ai:openai'), REAL_KEY);
    assert.deepEqual([...new Set(keyring.calls.map((call) => call.verb))], ['lookup']);
    assertNoValue('a refused run', events, refusal.message, keyring.calls);
  });

  test(`${platform.name}: with no entry the run is cleared, and the planted entry is deleted after a passing run`, () => {
    const keyring = fakeKeyring();
    const store = platform.store(keyring);
    const { events, record } = trace();

    checkNoAiKeys([store], record);
    assert.deepEqual(events, [{
      machine: platform.name, event: 'ai-keys-checked', service: SERVICE, entries: { 'ai:openai': { existed: false } },
    }]);

    // The walkthrough runs: the app moves the planted key into the keyring.
    keyring.entries.set('ai:openai', PLANTED_KEY);
    const problems = removePlantedAiKeys(store, record);

    assert.deepEqual(problems, []);
    assert.equal(keyring.entries.size, 0);
    assert.deepEqual(events[1], {
      machine: platform.name, event: 'ai-keys-removed', service: SERVICE, entries: { 'ai:openai': { removed: true, left: false } },
    });
    assertNoValue('a passing run', events, keyring.calls);
  });

  test(`${platform.name}: a run that fails after planting still has its entry deleted`, async () => {
    const keyring = fakeKeyring();
    const store = platform.store(keyring);
    const { events, record } = trace();
    checkNoAiKeys([store], record);

    // The controller records a failed walkthrough and goes on to its cleanup, as runWalkthroughLocked
    // does: the cleanup takes no account of how the walkthrough ended.
    let runError;
    try {
      await (async () => {
        keyring.entries.set('ai:openai', PLANTED_KEY);
        throw new Error('step failed after the key was planted');
      })();
    } catch (error) {
      runError = error;
    }
    const problems = removePlantedAiKeys(store, record);

    assert.equal(runError.message, 'step failed after the key was planted');
    assert.deepEqual(problems, []);
    assert.equal(keyring.entries.has('ai:openai'), false);
    assert.deepEqual(events.at(-1).entries, { 'ai:openai': { removed: true, left: false } });
    assertNoValue('a failed run', events, runError.message, keyring.calls);
  });

  test(`${platform.name}: a run that fails before the app stored the key finds nothing to delete`, () => {
    const keyring = fakeKeyring();
    const store = platform.store(keyring);
    const { events, record } = trace();
    checkNoAiKeys([store], record);

    assert.deepEqual(removePlantedAiKeys(store, record), []);
    assert.deepEqual(events.at(-1).entries, { 'ai:openai': { removed: false, left: false } });
  });

  test(`${platform.name}: an entry that cannot be deleted is reported as left`, () => {
    const keyring = fakeKeyring({ 'ai:openai': PLANTED_KEY });
    const store = platform.store(keyring, { clears: false });
    const { events, record } = trace();

    const problems = removePlantedAiKeys(store, record);

    assert.equal(problems.length, 1);
    assert.match(problems[0], new RegExp(`ai:openai is still in the ${platform.name} keyring`));
    assert.deepEqual(events.at(-1).entries, { 'ai:openai': { removed: false, left: true } });
    assertNoValue('a failed cleanup', events, problems);
  });

  test(`${platform.name}: only the AI accounts the run plants are checked and removed`, () => {
    // Another AI entry and a Notion entry belong to the user and are none of this run's business.
    const keyring = fakeKeyring({ 'ai:anthropic': REAL_KEY, 'integration:notion:vault-1': REAL_KEY });
    const store = platform.store(keyring);
    const { events, record } = trace();

    checkNoAiKeys([store], record);
    keyring.entries.set('ai:openai', PLANTED_KEY);
    removePlantedAiKeys(store, record);

    assert.deepEqual([...keyring.entries.keys()].sort(), ['ai:anthropic', 'integration:notion:vault-1']);
    assert.deepEqual(new Set(keyring.calls.map((call) => call.account)), new Set(['ai:openai']));
    assert.deepEqual(Object.keys(events[0].entries), ['ai:openai']);
  });

  test(`${platform.name}: the stores refuse accounts that are not the app's AI keys`, () => {
    const keyring = fakeKeyring({ 'integration:notion:vault-1': REAL_KEY });
    const store = platform.store(keyring);
    for (const account of ['integration:notion:vault-1', 'ai:other', 'sync:webdav:v', '', 'AI:OPENAI']) {
      assert.throws(() => store.stored(account), /not an AI key account/);
      assert.throws(() => store.remove(account), /not an AI key account/);
    }
    assert.deepEqual(keyring.calls, [], 'a refused account never reaches the keyring');
    assert.equal(keyring.entries.get('integration:notion:vault-1'), REAL_KEY);
  });
}

test('fedora: the secret-tool output never reaches the harness', () => {
  const keyring = fakeKeyring({ 'ai:openai': REAL_KEY });
  const seen = [];
  const spawn = (command, args, options) => {
    seen.push(options.stdio);
    return fakeSecretTool(keyring)(command, args, options);
  };
  const store = fedoraAiKeyStore({ spawn });

  assert.equal(store.stored('ai:openai'), true);
  assert.deepEqual(store.remove('ai:openai'), { found: true, left: false });

  assert.equal(seen.length, 4, 'lookup, lookup, clear, lookup');
  for (const stdio of seen) assert.deepEqual(stdio, ['ignore', 'ignore', 'pipe'], 'stdout goes to the null device');
});

test('fedora: a secret-tool that failed is not read as an absent entry', () => {
  const failing = (status, stderr, error) => fedoraAiKeyStore({ spawn: () => ({ status, stdout: null, stderr, error }) });

  assert.throws(() => failing(1, 'secret-tool: Cannot autolaunch D-Bus without X11 $DISPLAY').stored('ai:openai'), /failed \(exit 1\).*D-Bus/);
  assert.throws(() => failing(127, '').stored('ai:openai'), /failed \(exit 127\)/);
  assert.throws(() => failing(null, '', new Error('spawn secret-tool ENOENT')).stored('ai:openai'), /ENOENT/);
  assert.equal(failing(1, '').stored('ai:openai'), false);
  assert.equal(failing(1, '  \n').stored('ai:openai'), false);
});

test('a failed removal is a problem for the run, never a silent pass', () => {
  const { events, record } = trace();
  const broken = { name: 'windows', remove() { throw new Error('the desktop script failed with 1'); } };

  const problems = removePlantedAiKeys(broken, record);

  assert.deepEqual(problems, ['ai:openai on windows: the desktop script failed with 1']);
  assert.deepEqual(events.at(-1).entries, { 'ai:openai': { removed: false, left: null } });
});

test('a keyring that does not answer yes or no stops the run', () => {
  const { record } = trace();
  assert.throws(() => checkNoAiKeys([{ name: 'fedora', stored: () => undefined }], record), /did not answer yes or no/);
});

test('both machines are checked before the run fails, and a refusal names each machine that holds an entry', () => {
  const fedora = fakeKeyring();
  const windows = fakeKeyring({ 'ai:openai': REAL_KEY });
  const stores = [PLATFORMS[0].store(fedora), PLATFORMS[1].store(windows)];
  const { events, record } = trace();

  assert.throws(() => checkNoAiKeys(stores, record), (error) => {
    assert.match(error.message, /windows \(account ai:openai/);
    assert.doesNotMatch(error.message, /fedora \(account/);
    return true;
  });
  assert.deepEqual(events.map((event) => [event.machine, event.entries['ai:openai'].existed]), [['fedora', false], ['windows', true]]);
  assert.equal(windows.entries.get('ai:openai'), REAL_KEY);

  fedora.entries.set('ai:openai', REAL_KEY);
  assert.throws(() => checkNoAiKeys(stores, trace().record), /fedora \(account ai:openai[^;]*; windows \(account ai:openai/);
});

test('the Windows worker passes on the two flags and nothing the script wrote beside them', () => {
  const keyring = fakeKeyring({ 'ai:openai': REAL_KEY });
  const desktop = fakeDesktopScript(keyring, { extra: { value: REAL_KEY, length: REAL_KEY.length } });

  const answer = windowsAiKeyAction({ account: 'ai:openai', remove: false }, { tools: TOOLS, ...desktop });

  assert.deepEqual(answer, { found: true, left: true });
  assert.equal(desktop.files.size, 0, 'the result file is deleted as soon as it is read');
  assertNoValue('the worker answer', answer);
});

test('the Windows worker deletes its result file when the script fails, and starts from none', () => {
  const keyring = fakeKeyring();
  const desktop = fakeDesktopScript(keyring);
  desktop.files.set(`${TOOLS}\\ai-key-result.json`, JSON.stringify({ found: true, left: true }));

  // A script that wrote nothing cannot be answered by the stale file: it was cleared first.
  assert.throws(
    () => windowsAiKeyAction({ account: 'ai:openai', remove: false }, { tools: TOOLS, ...desktop, runDesktop: () => {} }),
  );
  assert.equal(desktop.files.size, 0);

  desktop.files.set(`${TOOLS}\\ai-key-result.json`, JSON.stringify({ found: true, left: true }));
  assert.throws(
    () => windowsAiKeyAction({ account: 'ai:openai', remove: false }, {
      tools: TOOLS, ...desktop, runDesktop: () => { throw new Error('desktop script failed with 1'); },
    }),
    /desktop script failed/,
  );
  assert.equal(desktop.files.size, 0, 'none is left behind after a failure');

  assert.throws(
    () => windowsAiKeyAction({ account: 'ai:openai', remove: false }, { tools: TOOLS, ...desktop, readResult: () => ({ found: 'yes' }) }),
    /did not answer found\/left/,
  );
});

test('the Windows target is "<account>.<service>" for the four AI accounts and nothing else', () => {
  assert.equal(windowsAiKeyTarget('ai:openai'), `ai:openai.${SERVICE}`);
  assert.deepEqual(AI_KEY_ACCOUNTS.map(windowsAiKeyTarget), [
    `ai:anthropic.${SERVICE}`, `ai:openai.${SERVICE}`, `ai:ollama.${SERVICE}`, `ai:openai-compatible.${SERVICE}`,
  ]);
  assert.throws(() => windowsAiKeyTarget(`integration:notion:abc`), /not an AI key account/);
});

test('alpha-desktop.ps1 accepts exactly the app\'s AI targets besides the Notion token', () => {
  const script = readFileSync(new URL('../scripts/windows/alpha-desktop.ps1', import.meta.url), 'utf8');

  assert.equal(/\$appService = '([^']+)'/.exec(script)?.[1], SERVICE);
  const accounts = /\$aiAccounts = @\(([^)]*)\)/.exec(script)?.[1].split(',').map((account) => account.trim().replaceAll("'", ''));
  assert.deepEqual(accounts, AI_KEY_ACCOUNTS);
  assert.match(script, /\$aiTargets = foreach \(\$aiAccount in \$aiAccounts\) \{ '\{0\}\.\{1\}' -f \$aiAccount, \$appService \}/);
  // Exact, case-sensitive membership, and the Notion prefix as before; anything else throws.
  assert.match(script, /if \(-not \$isNotionToken -and \$aiTargets -cnotcontains \$CredentialTarget\) \{\s*throw /);
  assert.match(script, /\$isNotionToken = \$CredentialTarget\.StartsWith\('integration:notion:'\)/);
  // The only thing the mode writes is the two flags.
  assert.match(script, /\[pscustomobject\]@\{ found = \$credentialFound; left = \$credentialLeft \}/);
});

test('the walkthrough plants exactly the keys it checks for', () => {
  const patch = plantedAiKeyPatch(PLANTED_KEY);

  assert.deepEqual(Object.keys(patch), Object.keys(PLANTED_AI_KEYS));
  assert.deepEqual(Object.values(patch), [PLANTED_KEY]);
  for (const account of Object.values(PLANTED_AI_KEYS)) assert.ok(AI_KEY_ACCOUNTS.includes(account));
  assert.deepEqual(Object.values(PLANTED_AI_KEYS), ['ai:openai']);

  // With no accounts named, the check asks about the planted ones and no others.
  const asked = [];
  checkNoAiKeys([{ name: 'fedora', stored: (account) => { asked.push(account); return false; } }], trace().record);
  assert.deepEqual(asked, Object.values(PLANTED_AI_KEYS));
});
