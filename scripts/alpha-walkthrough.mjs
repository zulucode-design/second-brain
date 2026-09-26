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
  await browser.waitUntil(async () => {
    last = await browser.execute((css) => document.querySelector(css)?.innerText.trim() ?? '', selector);
    return predicate(last);
  }, { timeout, timeoutMsg: `${message}: ${last.slice(0, 300)}` });
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
  // Focus first, then select: typing replaces the selection instead of appending to it.
  await browser.execute((input) => {
    input.focus();
    input.setSelectionRange(0, input.value.length);
  }, titleInput);
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
  const title = 'Walkthrough capture';
  const first = `First edit ${MARKER}.`;
  const second = ' Second edit after reopening.';
  await createNote(browser, type, { category: 'Projects', title, body: first });
  await openCategory(browser, 'Areas');
  await openCategory(browser, 'Projects');
  await openNote(browser, title);
  const reopened = await editorText(browser);
  if (!reopened.includes(first)) fail(`first edit missing after reopening: ${reopened}`);
  await appendToNote(browser, type, second);
  return { title, relativePath: `Projects/${title}.md`, expected: [first, second.trim()] };
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
  await press(browser, dialog.$(`button*=${category}`));
  await browser.waitUntil(async () => (await editorText(browser)).includes(expectedText), {
    timeout: 60_000, timeoutMsg: `clipped note does not contain "${expectedText}"`,
  });
  return { url, category, title: await openTitle(browser) };
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
  return { path, dialog: 'bypassed: export_diagnostics invoked with the run path' };
}

export async function startupError(browser) {
  const error = browser.$('.error');
  await error.waitForDisplayed({ timeout: 60_000 });
  const text = await textOf(browser, error);
  if (!/config\.json is malformed/.test(text)) fail(`startup error does not name the malformed config: ${text}`);
  return { error: text };
}

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
    // Pressed directly: pressText would name the page in its error.
    const [option] = await byText(browser, 'button.option-btn', page);
    await press(browser, option);
    const toggle = browser.$('button[aria-label="Publish to Notion from this machine"]');
    await toggle.waitForExist({ timeout: 60_000 });
    if ((await toggle.getAttribute('aria-checked')) !== 'true') await press(browser, toggle);
    await pressTextWhenReady(browser, 'button.import-btn', 'Publish Now');
    const text = await waitForText(
      browser, 'body',
      (value) => /Last published:/.test(value) && !/Publishing/.test(value),
      'Notion publish did not finish', 10 * 60_000,
    );
    const summary = /Last published:[^\n]*(\n[^\n]*){0,2}/.exec(text)?.[0];
    const error = await browser.execute(() => document.querySelector('.import-result.error')?.innerText.trim() ?? null);
    if (error) fail(`Notion publish failed: ${error}`);
    // A note Notion refuses is counted, not raised, so the summary is where it shows (#152).
    if (/\d+ failed|failing to publish/.test(text)) fail(`Notion publish left notes unpublished: ${summary}`);
    return { summary, tokenStoredWhileConnected: stored };
  } catch (error) {
    failure = error;
    throw error;
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
