// In-window steps of the matrix v2 installed-package walkthrough (#137 Gate 3).
//
// Each step drives the installed app through tauri-driver and returns what it observed. The
// controller in alpha-harness.mjs owns machines, processes, and evidence; this module only
// knows the app's UI. Only the controller imports it, so webdriverio stays off the Windows copy.

export const MARKER = 'alpha-walkthrough';

function fail(message) {
  throw new Error(message);
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

// WebKitWebDriver's getText returns '' for many visible elements, so text is read in the page.
async function textOf(browser, element) {
  return browser.execute((target) => target.innerText.trim(), await element);
}

// Elements under `selector` whose trimmed innerText equals `text`, or starts with it when
// `prefix` is set (sidebar counts follow the category name).
async function byText(browser, selector, text, { prefix = false } = {}) {
  return elements(browser, browser.execute((css, wanted, startsWith) => [...document.querySelectorAll(css)].filter((element) => {
    const value = element.innerText.trim();
    return startsWith ? value === wanted || value.startsWith(`${wanted} `) || value.startsWith(`${wanted}\n`) : value === wanted;
  }), selector, text, prefix));
}

// execute() hands back bare element references; wrap them so they take element commands.
async function elements(browser, pending) {
  return Promise.all((await pending).map((reference) => browser.$(reference)));
}

// Notion's steps each wait on a network round trip before their next button exists.
async function pressTextWhenReady(browser, selector, text, timeout = 60_000) {
  await browser.waitUntil(async () => (await byText(browser, selector, text)).length === 1, {
    timeout, timeoutMsg: `no ${selector} reading "${text}" appeared`,
  });
  await pressText(browser, selector, text);
}

async function pressText(browser, selector, text, options) {
  const found = await byText(browser, selector, text, options);
  if (found.length !== 1) fail(`expected one ${selector} reading "${text}", found ${found.length}`);
  await press(browser, found[0]);
}

// Windows (msedgedriver) takes real key input. WebKitWebDriver rejects every key action, so
// Fedora types through the browser's editing path, which ProseMirror observes like typing.
//
// Both type at the caret the caller left: a click would collapse a selection the caller made to
// replace text, and would move the caret set for an append.
export function typist(realKeys) {
  return async (browser, pending, text) => {
    const element = await pending;
    await element.waitForDisplayed({ timeout: 30_000 });
    if (realKeys) {
      await browser.execute((target) => target.focus(), element);
      await browser.keys(text.split(''));
      return;
    }
    const inserted = await browser.execute((target, value) => {
      target.focus();
      return document.execCommand('insertText', false, value);
    }, element, text);
    if (!inserted) fail('the webview refused inserted text');
  };
}

// A title rename is committed when the input loses focus.
async function blurTitle(browser) {
  await browser.execute(() => document.querySelector('.ProseMirror').focus());
}

async function waitForText(browser, selector, predicate, message, timeout = 30_000) {
  let last = '';
  // timeoutMsg is fixed when the wait starts, so the text last seen is added when it ends.
  await browser.waitUntil(async () => {
    last = await browser.execute((css) => document.querySelector(css)?.innerText.trim() ?? '', selector);
    return predicate(last);
  }, { timeout, timeoutMsg: message }).catch((error) => fail(`${error.message}: ${last.slice(0, 300)}`));
  return last;
}

async function openCategory(browser, category) {
  const found = await byText(browser, 'button', category, { prefix: true });
  if (!found.length) fail(`sidebar has no ${category} category`);
  await press(browser, found[0]);
}

async function noteRows(browser, title) {
  return byText(browser, '.note-title', title);
}

// The list re-renders after a category change, so it is read once it settles.
async function waitForRows(browser, title, count, timeoutMsg) {
  await browser.waitUntil(async () => (await noteRows(browser, title)).length === count, { timeout: 15_000, timeoutMsg });
}

function openTitle(browser) {
  return browser.execute(() => document.querySelector('.editor-title input')?.value);
}

async function waitForTitle(browser, title, timeoutMsg) {
  await browser.waitUntil(async () => (await openTitle(browser)) === title, { timeout: 15_000, timeoutMsg });
}

async function openNote(browser, title) {
  await waitForRows(browser, title, 1, `expected one note titled "${title}" in the list`);
  await press(browser, (await noteRows(browser, title))[0]);
  await waitForTitle(browser, title, `note "${title}" did not open`);
}

export async function createNote(browser, type, { category, title, body }) {
  await press(browser, browser.$('button[title^="New Note"]'));
  await press(browser, browser.$('.creation-dialog').$(`button*=${category}`));
  const titleInput = await browser.$('.editor-title input');
  await titleInput.waitForDisplayed({ timeout: 15_000 });
  await browser.waitUntil(async () => (await titleInput.getValue()) === 'Untitled', { timeout: 15_000 });
  // Focus first, then select: typing replaces the selection instead of appending to it. The app
  // focuses and selects the title itself a tick after creating the note (Editor focusTitle), and
  // a note created right after another can still re-render it, collapsing the caret to the end.
  // So the selection has to hold for a moment before typing starts.
  const selected = () => browser.execute((input) => document.activeElement === input
    && input.selectionStart === 0 && input.selectionEnd === input.value.length, titleInput);
  await browser.waitUntil(async () => {
    await browser.execute((input) => {
      input.focus();
      input.setSelectionRange(0, input.value.length);
    }, titleInput);
    await browser.pause(300);
    return selected();
  }, { timeout: 15_000, timeoutMsg: 'the new note\'s title did not stay selected' });
  await type(browser, titleInput, title);
  await blurTitle(browser);
  await waitForTitle(browser, title, `title "${title}" was not kept`);
  if (body) await type(browser, browser.$('.ProseMirror'), body);
}

export async function vaultOpened(browser) {
  const sidebarRoots = () => browser.execute(() => [...document.querySelectorAll('button')]
    .map((button) => button.innerText.trim())
    .filter((text) => /^(Projects|Areas|Resources|Archives)\s+\d+$/.test(text)));
  const complete = (categories) => categories.map((text) => text.split(/\s+/)[0]).sort().join() === 'Archives,Areas,Projects,Resources';
  // The sidebar fills in after the New Note button that marks the app ready, so wait for it.
  let categories = [];
  await browser.waitUntil(async () => complete(categories = await sidebarRoots()), { timeout: 15_000 })
    .catch(() => fail(`PARA roots missing: ${JSON.stringify(categories)}`));
  // The banner loads after the sidebar, so the backend's repair status is asked directly too.
  const status = await browser.executeAsync((done) => {
    window.__TAURI_INTERNALS__.invoke('get_repair_status')
      .then((value) => done({ ok: true, value }), (error) => done({ ok: false, error: String(error) }));
  });
  if (!status.ok) fail(`repair status failed: ${status.error}`);
  const repair = await browser.execute(() => document.querySelector('.repair-banner')?.innerText ?? null);
  if (status.value.issues.length || repair) fail(`fresh vault shows repair issues: ${JSON.stringify({ issues: status.value.issues, repair })}`);
  return { categories, repairIssues: 0 };
}

function editorText(browser) {
  return browser.execute(() => document.querySelector('.ProseMirror')?.innerText.trim() ?? '');
}

// Types `text` at the end of the open note's body.
async function appendToNote(browser, type, text) {
  await browser.execute(() => {
    const editor = document.querySelector('.ProseMirror');
    editor.focus();
    const range = document.createRange();
    range.selectNodeContents(editor);
    range.collapse(false);
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);
  });
  await type(browser, browser.$('.ProseMirror'), text);
}

// Row A: edit, navigate away at once, reopen, edit again, then close without waiting. The
// controller checks the file on disk after the app has exited.
export async function saveLifecycle(browser, type) {
  const category = 'Projects';
  const title = 'Walkthrough capture';
  const first = `First edit ${MARKER}.`;
  const second = ' Second edit after reopening.';
  await createNote(browser, type, { category, title, body: first });
  await openCategory(browser, 'Areas');
  await openCategory(browser, category);
  await openNote(browser, title);
  const reopened = await editorText(browser);
  if (!reopened.includes(first)) fail(`first edit missing after reopening: ${reopened}`);
  await appendToNote(browser, type, second);
  return { category, title, relativePath: `${category}/${title}.md`, expected: [first, second.trim()] };
}

// Offline editing: an existing note takes an edit. The controller checks the file after exit.
export async function editNote(browser, type, { category, title, text }) {
  await openCategory(browser, category);
  await openNote(browser, title);
  await appendToNote(browser, type, text);
  await browser.waitUntil(async () => (await editorText(browser)).endsWith(text.trim()), {
    timeout: 15_000, timeoutMsg: `the edit to "${title}" did not reach the editor`,
  });
  return { title, appended: text.trim() };
}

// The close button runs the app's own save-aware shutdown.
export async function closeWindow(browser) {
  await browser.execute(() => document.querySelector('button[title="Close"]').click());
}

async function searchFor(browser, type, mode, query) {
  await press(browser, browser.$('button[title^="Search"]'));
  const input = browser.$('input[placeholder="Search notes..."]');
  await input.waitForDisplayed({ timeout: 10_000 });
  await pressText(browser, '.search-modes button', mode);
  await type(browser, input, query);
  // Results arrive after the input debounce; an empty list is reported, not waited out forever.
  await browser.waitUntil(
    async () => (await browser.execute(() => document.querySelectorAll('.result-title').length)) > 0,
    { timeout: 60_000 },
  ).catch(() => {});
  const titles = await browser.execute(() => [...document.querySelectorAll('.result-title')].map((element) => element.innerText.trim()));
  await browser.execute(() => document.querySelector('input[placeholder="Search notes..."]')
    .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
  return titles;
}

// `mode` is the search panel's toggle: Keyword or Semantic.
export async function search(browser, type, { mode, query, expected }) {
  const titles = await searchFor(browser, type, mode, query);
  if (!titles.includes(expected)) fail(`${mode.toLowerCase()} search for "${query}" did not find "${expected}": ${JSON.stringify(titles)}`);
  return { query, titles };
}

async function openSettingsTab(browser, tab) {
  await press(browser, browser.$('button[title="Settings"]'));
  await pressText(browser, 'button.tab-btn', tab);
}

export async function closeSettings(browser) {
  await browser.execute(() => document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
}

// The semantic index fills in the background; the Maintenance tab reports its progress.
export async function waitForSemanticIndex(browser, timeout = 10 * 60_000) {
  await openSettingsTab(browser, 'Maintenance');
  const text = await waitForText(
    browser,
    'body',
    (value) => /Indexed notes\s*[1-9]\d*/.test(value) && /Waiting for embedding\s*0\b/.test(value),
    'semantic index did not finish',
    timeout,
  );
  await closeSettings(browser);
  return { status: /Indexed notes\s*\d+\s*Waiting for embedding\s*\d+/.exec(text)?.[0].replaceAll(/\s+/g, ' ') };
}

export async function graph(browser) {
  await press(browser, browser.$('button[title="Graph View"]'));
  const stats = await waitForText(browser, '.graph-stats', (value) => /\d/.test(value), 'graph did not report its size');
  const canvas = await browser.execute(() => {
    const element = document.querySelector('.graph-panel canvas');
    return element ? { width: element.width, height: element.height } : null;
  });
  if (!canvas?.width || !canvas.height) fail('graph drew no canvas');
  return { stats, canvas };
}

export async function closeGraph(browser) {
  await press(browser, browser.$('button.graph-close'));
}

export async function tasks(browser, { text }) {
  await openCategory(browser, 'Tasks');
  const openRows = (wanted) => browser.execute((value) => [...document.querySelectorAll('.task-row:not(.done)')]
    .filter((element) => element.innerText.includes(value)).length, wanted);
  await browser.waitUntil(async () => (await openRows(text)) === 1, {
    timeout: 30_000, timeoutMsg: `Tasks view does not list "${text}" as open`,
  });
  const [row] = await elements(browser, browser.execute((value) => [...document.querySelectorAll('.task-row:not(.done)')]
    .filter((element) => element.innerText.includes(value)), text));
  await press(browser, row.$('button.task-check'));
  // "Hide done" is on by default, so a completed task leaves the open list.
  await browser.waitUntil(async () => (await openRows(text)) === 0, {
    timeout: 15_000, timeoutMsg: 'task did not turn done',
  });
  return { text, completed: true };
}

// Opens the note's row menu (a right-click in the list) and chooses `action`.
async function noteMenu(browser, category, title, action) {
  await openCategory(browser, category);
  await openNote(browser, title);
  const [row] = await noteRows(browser, title);
  await browser.execute((target) => target.dispatchEvent(new MouseEvent('contextmenu', {
    bubbles: true, cancelable: true, clientX: 200, clientY: 200,
  })), row);
  await pressText(browser, 'button', action);
}

export async function moveNote(browser, { from, to, title }) {
  await noteMenu(browser, from, title, 'Move to...');
  await pressText(browser, '.move-picker-list button', to);
  await openCategory(browser, to);
  await waitForRows(browser, title, 1, `"${title}" did not arrive in ${to}`);
  return { title, from, to };
}

export async function trashAndRestore(browser, { category, title }) {
  await noteMenu(browser, category, title, 'Move to Trash');
  await openCategory(browser, 'Trash');
  await waitForRows(browser, title, 1, `"${title}" is not in the trash`);
  const [trashed] = await noteRows(browser, title);
  const restored = await browser.execute((target) => {
    const button = target.closest('.note-item')?.querySelector('.trash-row-btn[title="Restore"]');
    button?.click();
    return Boolean(button);
  }, trashed);
  if (!restored) fail('trash row has no Restore action');
  await waitForRows(browser, title, 0, `"${title}" stayed in the trash`);
  await openCategory(browser, category);
  await waitForRows(browser, title, 1, `"${title}" did not return to ${category}`);
  return { title, restored: true, category };
}

export async function noteHistory(browser, type, { category, title }) {
  await openCategory(browser, category);
  await openNote(browser, title);
  await press(browser, browser.$('button[title="Version history"]'));
  await press(browser, browser.$('button.history-create-btn'));
  await browser.$('.history-item').waitForDisplayed({ timeout: 15_000 });
  const before = await editorText(browser);
  const edit = ' Edit made after the version.';
  await appendToNote(browser, type, edit);
  // Without the edit on screen, a restore that did nothing would still match `before`.
  await browser.waitUntil(async () => (await editorText(browser)).endsWith(edit.trim()), {
    timeout: 15_000, timeoutMsg: 'the edit after the version did not reach the editor',
  });
  const items = await browser.$$('.history-item');
  await press(browser, items[0]);
  await press(browser, browser.$('button.history-restore-btn'));
  await browser.waitUntil(
    async () => (await editorText(browser)) === before,
    { timeout: 15_000, timeoutMsg: 'restoring the version did not bring back its text' },
  );
  return { versions: items.length, restoredText: before };
}

export async function backupNow(browser) {
  await openSettingsTab(browser, 'Backup');
  await press(browser, browser.$('button.backup-link-btn*=Backup now'));
  const text = await waitForText(browser, '.import-result', (value) => value.length > 0, 'backup reported nothing', 5 * 60_000);
  if (!text.includes('Backup created successfully')) fail(`backup failed in the app: ${text}`);
  return { message: text };
}

export async function restoreLatest(browser) {
  await openSettingsTab(browser, 'Backup');
  const restoreButtons = await browser.$$('button.backup-action-btn[title="Restore"]');
  if (!restoreButtons.length) fail('no backup to restore');
  await press(browser, restoreButtons[0]);
  await press(browser, browser.$('button.restore-confirm-btn'));
  const text = await waitForText(
    browser, '.import-result', (value) => /restored|failed/i.test(value), 'restore did not report a result', 5 * 60_000,
  );
  if (text !== 'Backup restored.') fail(`restore did not succeed in the app: ${text}`);
  return { message: text };
}

export async function clipPage(browser, type, { url, category, expectedText }) {
  await press(browser, browser.$('button[title="Clip web page"]'));
  const dialog = browser.$('[role="dialog"]');
  await dialog.waitForDisplayed({ timeout: 10_000 });
  await type(browser, dialog.$('input'), url);
  // The route from the Windows test machine to the page stalled past the app's 20 s limit in one
  // of 40 plain fetches (2026-09-26). The app then says so and keeps the dialog open, as a user
  // would see it, so one retry is allowed on exactly that message (src-tauri/src/web_clipping.rs),
  // and the result records it. Any other error fails at once, naming itself.
  const timedOut = 'The web page took too long to respond';
  let retried = false;
  for (;;) {
    await press(browser, dialog.$(`button*=${category}`));
    let outcome;
    await browser.waitUntil(async () => {
      if ((await editorText(browser)).includes(expectedText)) outcome = 'clipped';
      else outcome = await browser.execute(() => document.querySelector('[role="dialog"] [role="alert"]')?.innerText || null);
      return outcome;
    }, { timeout: 60_000, timeoutMsg: `clipped note does not contain "${expectedText}"` });
    if (outcome === 'clipped') break;
    if (!outcome.startsWith(timedOut)) fail(`the clip failed: ${outcome}`);
    if (retried) fail(`the clip timed out twice: ${outcome}`);
    retried = true;
  }
  return { url, category, title: await openTitle(browser), retriedAfterTimeout: retried };
}

// The file picker is a native dialog WebDriver cannot answer, so the file reaches the editor's
// own hidden input instead. WebView2 ignores a synthetic DataTransfer, so Windows sends the
// staged file's path the way WebDriver uploads a file; WebKitGTK ignores that and takes the
// DataTransfer.
export async function attachFile(browser, { name, content, path }) {
  const input = await browser.$('#insert-file-input');
  // The app logs a failed attachment and shows nothing, so its console is the only report.
  await browser.execute(() => {
    window.__attachErrors = [];
    const original = console.error;
    console.error = (...parts) => {
      window.__attachErrors.push(parts.map((part) => part?.message ?? String(part)).join(' '));
      original(...parts);
    };
  });
  let delivered;
  if (path) {
    // Element send keys reaches a hidden input, but not a display:none one.
    await browser.execute((target) => { target.style.display = ''; }, input);
    await input.addValue(path);
    await browser.execute((target) => { target.style.display = 'none'; }, input);
    delivered = 'path';
  } else {
    delivered = await browser.execute((target, fileName, text) => {
      const transfer = new DataTransfer();
      transfer.items.add(new File([text], fileName, { type: 'text/plain' }));
      target.files = transfer.files;
      // Checked before the change event: the app's handler empties the input once it has the file.
      if (target.files.length !== 1) return 'refused';
      target.dispatchEvent(new Event('change', { bubbles: true }));
      return 'data-transfer';
    }, input, name, content);
    if (delivered === 'refused') fail('the webview refused the file list');
  }
  try {
    await browser.waitUntil(async () => (await editorText(browser)).includes(name), { timeout: 30_000 });
  } catch {
    const logged = await browser.execute(() => window.__attachErrors ?? []);
    fail(`attachment ${name} did not appear in the note (${delivered}); console: ${JSON.stringify(logged)}`);
  }
  return { name, delivered };
}

// The export button opens a native save dialog, which WebDriver cannot answer. The harness shows
// the button, then calls its own command with the path the dialog would have returned.
export async function exportDiagnostics(browser, path) {
  await openSettingsTab(browser, 'Maintenance');
  const [button] = await byText(browser, 'button', 'Export diagnostics');
  if (!button || !(await button.isEnabled())) fail('Settings > Maintenance has no enabled Export diagnostics button');
  const result = await browser.executeAsync((target, done) => {
    window.__TAURI_INTERNALS__.invoke('export_diagnostics', { path: target })
      .then(() => done({ ok: true }), (error) => done({ ok: false, error: String(error) }));
  }, path);
  if (!result.ok) fail(`diagnostic export failed: ${result.error}`);
  return { path, button: 'enabled', dialog: 'bypassed: export_diagnostics invoked with the run path' };
}

export async function startupError(browser) {
  const error = browser.$('.error');
  await error.waitForDisplayed({ timeout: 60_000 });
  const text = await textOf(browser, error);
  if (!/config\.json is malformed/.test(text)) fail(`startup error does not name the malformed config: ${text}`);
  return { error: text };
}

// Long enough for a page or database that did land to become visible before a second try.
const RETRY_PAUSE_MS = 20_000;

// Connects this run's vault to a disposable Notion page, publishes once, then disconnects so
// the token leaves the machine's keyring. The controller checks Notion itself afterwards. The
// page title is a secret in the workflow, so neither the result nor an error names it.
//
// `whileConnected` (required) runs once the token is stored, so the controller can prove its
// keyring lookup finds it; otherwise finding nothing after the disconnect would prove nothing.
export async function notionPublish(browser, type, { token, page }, whileConnected) {
  await openSettingsTab(browser, 'Notion');
  await type(browser, browser.$('input[placeholder="ntn_…"]'), token);
  await pressText(browser, 'button.import-btn', 'Connect');
  let failure;
  try {
    await pressTextWhenReady(browser, 'button.import-btn', 'Choose a page');
    const stored = await whileConnected();
    await browser.waitUntil(async () => (await byText(browser, 'button.option-btn', page)).length === 1, {
      timeout: 60_000, timeoutMsg: 'the disposable Notion page is not shared with the integration',
    });
    const toggle = browser.$('button[aria-label="Publish to Notion from this machine"]');
    const shownError = () => browser.execute(() => document.querySelector('.import-result.error')?.innerText.trim() ?? null);
    // A create whose connection was lost and that cannot be confirmed fails its attempt and is
    // resolved by the next one, never duplicated (#186). So the page is chosen, and the
    // publish run, at most twice. The second try comes after a pause that lets anything that
    // landed become visible. The controller's own Notion check still requires exactly one
    // database per category and one page per note.
    let setupRetriedAfter = null;
    for (;;) {
      // Pressed directly: pressText would name the page in its error.
      const [option] = await byText(browser, 'button.option-btn', page);
      if (!option) fail('the disposable Notion page is no longer offered');
      await press(browser, option);
      // Publish Now exists only once every database is recorded; the toggle is there as soon
      // as Notion is connected, so it says nothing about setup.
      let outcome;
      await browser.waitUntil(async () => {
        const ready = (await byText(browser, 'button.import-btn', 'Publish Now')).length === 1;
        outcome = ready ? 'set up' : await shownError();
        return Boolean(outcome);
      }, { timeout: 120_000, timeoutMsg: 'Notion setup did not finish' });
      if (outcome === 'set up') break;
      if (setupRetriedAfter || !/could not be confirmed/.test(outcome)) fail(`Notion setup failed: ${outcome}`);
      setupRetriedAfter = outcome;
      await new Promise((resolveWait) => setTimeout(resolveWait, RETRY_PAUSE_MS));
    }
    await toggle.waitForExist({ timeout: 60_000 });
    if ((await toggle.getAttribute('aria-checked')) !== 'true') await press(browser, toggle);
    const publishOnce = async (previous) => {
      await pressTextWhenReady(browser, 'button.import-btn', 'Publish Now');
      const text = await waitForText(
        browser, 'body',
        (value) => /Last published:/.test(value) && !/Publishing/.test(value)
          && /Last published:[^\n]*/.exec(value)?.[0] !== previous,
        'Notion publish did not finish', 10 * 60_000,
      );
      return { text, summary: /Last published:[^\n]*(\n[^\n]*){0,2}/.exec(text)?.[0] };
    };
    // A note Notion refuses is counted, not raised, so the summary is where it shows (#152).
    const leftUnpublished = (text) => /\d+ failed|failing to publish/.test(text);
    let { text, summary } = await publishOnce(null);
    let publishRetriedAfter = null;
    if (!(await shownError()) && leftUnpublished(text)) {
      publishRetriedAfter = summary;
      await new Promise((resolveWait) => setTimeout(resolveWait, RETRY_PAUSE_MS));
      ({ text, summary } = await publishOnce(/Last published:[^\n]*/.exec(text)?.[0]));
    }
    const error = await shownError();
    if (error) fail(`Notion publish failed: ${error}`);
    if (leftUnpublished(text)) fail(`Notion publish left notes unpublished: ${summary}`);
    return { summary, tokenStoredWhileConnected: stored, setupRetriedAfter, publishRetriedAfter };
  } catch (error) {
    // Read now: the finally block disconnects and closes Settings, and a setup error is shown
    // nowhere else (#185).
    const shown = await browser.execute(() => [...document.querySelectorAll('.import-result.error')]
      .map((element) => element.innerText.trim()).filter(Boolean).join(' | ')).catch(() => '');
    failure = shown ? new Error(`${error.message}; Settings showed: ${shown}`) : error;
    throw failure;
  } finally {
    // Also after a failure, so the token does not stay in the keyring; the controller checks.
    // Disconnect appears once the connection settles; without one there is nothing to remove.
    const connected = await browser.waitUntil(
      async () => (await byText(browser, 'button.import-btn', 'Disconnect')).length === 1,
      { timeout: 30_000 },
    ).then(() => true, () => false);
    try {
      if (connected) {
        await pressText(browser, 'button.import-btn', 'Disconnect');
        await browser.waitUntil(async () => (await byText(browser, 'button.import-btn', 'Connect')).length === 1, {
          timeout: 30_000, timeoutMsg: 'Notion did not disconnect',
        });
      }
      await closeSettings(browser);
    } catch (error) {
      // The publish failure is the cause worth reporting; the controller clears the token anyway.
      if (!failure) throw error;
    }
  }
}

// ── Device sync (#28 acceptance gate) ──

const backend = (browser, command, args = {}) => browser.executeAsync((name, values, done) => {
  window.__TAURI_INTERNALS__.invoke(name, values)
    .then((value) => done({ ok: true, value }), (error) => done({ ok: false, error: String(error) }));
}, command, args);

// Read-only: the controller checks what the Settings screen reports against the backend's view.
export async function syncStatus(browser) {
  const result = await backend(browser, 'sync_status');
  if (!result.ok) fail(`sync status failed: ${result.error}`);
  return result.value;
}

// Settings may already be open from the previous step; pressing its button again would close it.
export async function openSync(browser) {
  if (await browser.execute(() => Boolean(document.querySelector('.settings-overlay')))) await pressText(browser, 'button.tab-btn', 'Sync');
  else await openSettingsTab(browser, 'Sync');
  await browser.$('button[aria-label="Enable sync on this machine"]').waitForDisplayed({ timeout: 15_000 });
}

// Flips the Settings toggle and waits for the backend to agree. Sync stays on the Sync tab.
export async function setSync(browser, enabled) {
  await openSync(browser);
  const toggle = browser.$('button[aria-label="Enable sync on this machine"]');
  if ((await toggle.getAttribute('aria-checked')) !== String(enabled)) await press(browser, toggle);
  await browser.waitUntil(async () => (await toggle.getAttribute('aria-checked')) === String(enabled), {
    timeout: 60_000, timeoutMsg: `sync toggle did not turn ${enabled ? 'on' : 'off'}`,
  });
  const status = await syncStatus(browser);
  if (status.enabled !== enabled) fail(`backend sync enabled=${status.enabled}, toggle says ${enabled}`);
  if (enabled) {
    await browser.waitUntil(async () => Boolean((await syncStatus(browser)).deviceId), {
      timeout: 60_000, timeoutMsg: 'no device ID appeared after enabling sync',
    });
  }
  return syncStatus(browser);
}

// What Settings shows for this machine: the values a person would copy to the other one.
export async function syncIdentity(browser) {
  await openSync(browser);
  return browser.execute(() => {
    const deviceId = document.querySelector('input[aria-label="This device ID"]')?.value ?? null;
    const inputs = [...document.querySelectorAll('.tab-content input.ai-key-input[readonly]')].map((element) => element.value);
    return { deviceId, vaultId: inputs.find((value) => value !== deviceId) ?? null };
  });
}

async function fillInput(browser, type, placeholder, value) {
  const input = await browser.$(`input[placeholder="${placeholder}"]`);
  await input.waitForDisplayed({ timeout: 15_000 });
  await browser.execute((target) => { target.focus(); target.select(); }, input);
  await type(browser, input, value);
  await browser.waitUntil(async () => (await input.getValue()) === value, {
    timeout: 10_000, timeoutMsg: `${placeholder} did not take "${value}"`,
  });
}

// Fills the Pair Another Device form and presses Pair explicitly. Returns Settings' message.
export async function pairDevice(browser, type, { name, deviceId, tailscaleIp, vaultId }) {
  await openSync(browser);
  await fillInput(browser, type, 'Device name', name);
  await fillInput(browser, type, 'Syncthing device ID', deviceId);
  await fillInput(browser, type, 'Tailscale IPv4 (100.x.x.x)', tailscaleIp);
  await fillInput(browser, type, 'Vault ID from the other machine', vaultId);
  await pressText(browser, 'button.import-btn', 'Pair explicitly');
  const text = await waitForText(browser, '.tab-content .import-result', (value) => value.length > 0, 'pairing reported nothing', 60_000);
  return { message: text, status: await syncStatus(browser) };
}

// Presses Sync now. The run ends with a sync-done event, which Settings shows as its message.
export async function pressSyncNow(browser) {
  await openSync(browser);
  await browser.waitUntil(async () => (await byText(browser, 'button.import-btn', 'Sync now')).length === 1, {
    timeout: 60_000, timeoutMsg: 'Sync now is not available',
  });
  await browser.execute(() => {
    window.__syncDone = null;
    window.__TAURI_INTERNALS__.invoke('plugin:event|listen', {
      event: 'sync-done', target: { kind: 'Any' },
      handler: window.__TAURI_INTERNALS__.transformCallback((event) => { window.__syncDone = event.payload; }),
    });
  });
  await pressText(browser, 'button.import-btn', 'Sync now');
}

// Waits for the run started by pressSyncNow and returns its terminal outcome.
export async function syncDone(browser, timeout = 8 * 60_000) {
  let payload = null;
  await browser.waitUntil(async () => (payload = await browser.execute(() => window.__syncDone)) !== null, {
    timeout, timeoutMsg: 'sync run did not finish',
  });
  const message = await waitForText(browser, '.tab-content .import-result', (value) => !/Creating a safety backup/.test(value), 'Settings kept the in-progress message', 30_000);
  return { ...payload, message };
}

export async function listConflicts(browser) {
  const result = await backend(browser, 'list_sync_conflicts');
  if (!result.ok) fail(`listing conflicts failed: ${result.error}`);
  await openSync(browser);
  const shown = await browser.execute(() => [...document.querySelectorAll('details.backup-item')].map((item) => ({
    relativePath: item.querySelector('summary')?.innerText.trim(),
    versions: [...item.querySelectorAll('pre.sync-preview')].map((element) => element.innerText),
  })));
  return { conflicts: result.value, shown };
}

// Chooses `choice` ('Keep current' or 'Use conflict') for the conflict on `relativePath`.
export async function resolveConflict(browser, relativePath, choice) {
  await openSync(browser);
  const resolved = await browser.execute((path, label) => {
    const item = [...document.querySelectorAll('details.backup-item')].find((element) => element.querySelector('summary')?.innerText.trim() === path);
    if (!item) return 'missing';
    item.open = true;
    const button = [...item.querySelectorAll('button.option-btn')].find((element) => element.innerText.trim() === label);
    if (!button || button.disabled) return 'no-button';
    button.click();
    return 'pressed';
  }, relativePath, choice);
  if (resolved !== 'pressed') fail(`conflict ${relativePath}: ${resolved} (${choice})`);
  const pressedAt = Date.now();
  // A choice waits for the note lease, which a scheduled run holds through its peer wait,
  // convergence, and handoff hold (docs/syncthing-sidecar.md), so it can take minutes. That
  // run's own result can replace the message in Settings, so the entry leaving the list is what
  // shows the choice was applied; Settings lists conflicts again only after a choice succeeds.
  let shown = null;
  await browser.waitUntil(async () => {
    shown = await browser.execute((path) => ({
      listed: [...document.querySelectorAll('details.backup-item summary')].some((element) => element.innerText.trim() === path),
      message: document.querySelector('.tab-content .import-result')?.innerText.trim() ?? '',
    }), relativePath);
    return !shown.listed;
  }, { timeout: 8 * 60_000, timeoutMsg: 'conflict choice was not applied' })
    .catch((error) => fail(`${error.message}: ${shown?.message.slice(0, 300)}`));
  return { relativePath, choice, message: shown.message, ms: Date.now() - pressedAt };
}

// The sidebar's PARA counts, as shown.
export async function paraCounts(browser) {
  return browser.execute(() => Object.fromEntries([...document.querySelectorAll('button')]
    .map((button) => /^(Projects|Areas|Resources|Archives)\s+(\d+)$/.exec(button.innerText.trim()))
    .filter(Boolean)
    .map((match) => [match[1], Number(match[2])])));
}

// Titles listed in a category, as shown.
export async function categoryTitles(browser, category) {
  await openCategory(browser, category);
  await browser.pause(1_000);
  return browser.execute(() => [...document.querySelectorAll('.note-title')].map((element) => element.innerText.trim()));
}

export async function graphNodes(browser) {
  const { stats } = await graph(browser);
  const result = await backend(browser, 'get_graph_data');
  await closeGraph(browser);
  return { stats, notes: Number(/(\d+) notes/.exec(stats)?.[1]), graph: result.ok ? result.value : { error: result.error } };
}

export async function keywordTitles(browser, type, query) {
  return searchFor(browser, type, 'Keyword', query);
}

export async function openNoteIn(browser, category, title) {
  await openCategory(browser, category);
  await openNote(browser, title);
  return { title, text: await editorText(browser) };
}

// The open note's vault images: whether each loaded, and whether the editor marked it pending.
export async function editorImages(browser) {
  return browser.execute(() => [...document.querySelectorAll('.ProseMirror img')].map((image) => ({
    src: image.getAttribute('src'),
    complete: image.complete,
    naturalWidth: image.naturalWidth,
    pending: image.dataset.syncPending === 'true',
    alt: image.alt,
  })));
}

// Like attachFile, through the image input, so the editor inserts an <img>.
export async function attachImage(browser, { name, base64, path }) {
  const input = await browser.$('#insert-image-input');
  let delivered;
  if (path) {
    await browser.execute((target) => { target.style.display = ''; }, input);
    await input.addValue(path);
    await browser.execute((target) => { target.style.display = 'none'; }, input);
    delivered = 'path';
  } else {
    delivered = await browser.execute((target, fileName, data) => {
      const bytes = Uint8Array.from(atob(data), (character) => character.charCodeAt(0));
      const transfer = new DataTransfer();
      transfer.items.add(new File([bytes], fileName, { type: 'image/png' }));
      target.files = transfer.files;
      if (target.files.length !== 1) return 'refused';
      target.dispatchEvent(new Event('change', { bubbles: true }));
      return 'data-transfer';
    }, input, name, base64);
    if (delivered === 'refused') fail('the webview refused the image');
  }
  await browser.waitUntil(async () => (await editorImages(browser)).some((image) => image.naturalWidth > 0), {
    timeout: 30_000, timeoutMsg: `image ${name} did not render in the note (${delivered})`,
  });
  return { name, delivered, images: await editorImages(browser) };
}

// A clip that must fail: the app has no route to the page. Returns the dialog's message and
// closes the dialog the way Escape does.
export async function clipFails(browser, type, { url, category }) {
  await press(browser, browser.$('button[title="Clip web page"]'));
  const dialog = browser.$('[role="dialog"]');
  await dialog.waitForDisplayed({ timeout: 10_000 });
  await type(browser, dialog.$('input'), url);
  await press(browser, dialog.$(`button*=${category}`));
  let message = null;
  await browser.waitUntil(async () => (message = await browser.execute(() => document.querySelector('[role="dialog"] [role="alert"]')?.innerText || null)) !== null, {
    timeout: 60_000, timeoutMsg: `the clip of ${url} neither failed nor finished`,
  });
  await browser.execute(() => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', code: 'Escape', bubbles: true })));
  await dialog.waitForDisplayed({ reverse: true, timeout: 10_000 });
  return { url, message };
}

export { appendToNote, closeSettings as closeSettingsPanel, openCategory };
