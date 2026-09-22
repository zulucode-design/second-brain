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
export async function press(browser, pending) {
  const element = await pending;
  await element.waitForDisplayed({ timeout: 30_000 });
  // A DOM click on a disabled button is silently ignored.
  await element.waitForEnabled({ timeout: 30_000 });
  await browser.execute((target) => target.click(), element);
}

// WebKitWebDriver's getText returns '' for many visible elements, so text is read in the page.
export async function textOf(browser, element) {
  return browser.execute((target) => target.innerText.trim(), await element);
}

// Elements under `selector` whose trimmed innerText equals `text`, or starts with it when
// `prefix` is set (sidebar counts follow the category name).
export async function byText(browser, selector, text, { prefix = false } = {}) {
  return elements(browser, browser.execute((css, wanted, startsWith) => [...document.querySelectorAll(css)].filter((element) => {
    const value = element.innerText.trim();
    return startsWith ? value === wanted || value.startsWith(`${wanted} `) || value.startsWith(`${wanted}\n`) : value === wanted;
  }), selector, text, prefix));
}

// execute() hands back bare element references; wrap them so they take element commands.
async function elements(browser, pending) {
  return Promise.all((await pending).map((reference) => browser.$(reference)));
}

export async function pressText(browser, selector, text, options) {
  const found = await byText(browser, selector, text, options);
  if (found.length !== 1) fail(`expected one ${selector} reading "${text}", found ${found.length}`);
  await press(browser, found[0]);
}

// Windows (msedgedriver) takes real key input. WebKitWebDriver rejects every key action, so
// Fedora types through the browser's editing path, which ProseMirror observes like typing.
export function typist(realKeys) {
  return async (browser, pending, text) => {
    const element = await pending;
    await element.waitForDisplayed({ timeout: 30_000 });
    if (realKeys) {
      await element.click();
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

// The list re-renders after a category change, so it is read once it shows the note.
async function openNote(browser, title) {
  await browser.waitUntil(async () => (await noteRows(browser, title)).length === 1, {
    timeout: 15_000, timeoutMsg: `expected one note titled "${title}" in the list`,
  });
  await press(browser, (await noteRows(browser, title))[0]);
  await browser.waitUntil(
    async () => (await browser.execute(() => document.querySelector('.editor-title input')?.value)) === title,
    { timeout: 15_000, timeoutMsg: `note "${title}" did not open` },
  );
}

export async function createNote(browser, type, { category, title, body }) {
  await press(browser, browser.$('button[title^="New Note"]'));
  await press(browser, browser.$('.creation-dialog').$(`button*=${category}`));
  const titleInput = await browser.$('.editor-title input');
  await titleInput.waitForDisplayed({ timeout: 15_000 });
  await browser.waitUntil(async () => (await titleInput.getValue()) === 'Untitled', { timeout: 15_000 });
  await browser.execute((input) => input.select(), titleInput);
  await type(browser, titleInput, title);
  await blurTitle(browser);
  await browser.waitUntil(
    async () => (await browser.execute(() => document.querySelector('.editor-title input')?.value)) === title,
    { timeout: 15_000, timeoutMsg: `title "${title}" was not kept` },
  );
  if (body) await type(browser, browser.$('.ProseMirror'), body);
}

export async function vaultOpened(browser) {
  const categories = await browser.execute(() => [...document.querySelectorAll('button')]
    .map((button) => button.innerText.trim())
    .filter((text) => /^(Projects|Areas|Resources|Archives)\s+\d+$/.test(text)));
  const roots = categories.map((text) => text.split(/\s+/)[0]).sort();
  if (roots.join() !== 'Archives,Areas,Projects,Resources') fail(`PARA roots missing: ${JSON.stringify(categories)}`);
  const repair = await browser.execute(() => document.querySelector('.repair-banner')?.innerText ?? null);
  if (repair) fail(`fresh vault shows repair issues: ${repair}`);
  return { categories, repair };
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
  const reopened = await browser.execute(() => document.querySelector('.ProseMirror').innerText.trim());
  if (!reopened.includes(first)) fail(`first edit missing after reopening: ${reopened}`);
  await browser.execute(() => {
    const editor = document.querySelector('.ProseMirror');
    editor.focus();
    const range = document.createRange();
    range.selectNodeContents(editor);
    range.collapse(false);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
  });
  await type(browser, browser.$('.ProseMirror'), second);
  return { title, relativePath: `Projects/${title}.md`, expected: [first, second.trim()] };
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

export async function keywordSearch(browser, type, { query, expected }) {
  const titles = await searchFor(browser, type, 'Keyword', query);
  if (!titles.includes(expected)) fail(`keyword search for "${query}" did not find "${expected}": ${JSON.stringify(titles)}`);
  return { query, titles };
}

export async function semanticSearch(browser, type, { query, expected }) {
  const titles = await searchFor(browser, type, 'Semantic', query);
  if (!titles.includes(expected)) fail(`semantic search for "${query}" did not find "${expected}": ${JSON.stringify(titles)}`);
  return { query, titles };
}

export async function openSettingsTab(browser, tab) {
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
  await browser.waitUntil(async () => (await noteRows(browser, title)).length === 1, {
    timeout: 15_000, timeoutMsg: `"${title}" did not arrive in ${to}`,
  });
  return { title, from, to };
}

export async function trashAndRestore(browser, { category, title }) {
  await noteMenu(browser, category, title, 'Move to Trash');
  await openCategory(browser, 'Trash');
  await browser.waitUntil(async () => (await noteRows(browser, title)).length === 1, {
    timeout: 15_000, timeoutMsg: `"${title}" is not in the trash`,
  });
  const [trashed] = await noteRows(browser, title);
  const restored = await browser.execute((target) => {
    const button = target.closest('.note-item')?.querySelector('.trash-row-btn[title="Restore"]');
    button?.click();
    return Boolean(button);
  }, trashed);
  if (!restored) fail('trash row has no Restore action');
  await browser.waitUntil(async () => (await noteRows(browser, title)).length === 0, {
    timeout: 15_000, timeoutMsg: `"${title}" stayed in the trash`,
  });
  return { title, restored: true };
}

export async function noteHistory(browser, type, { category, title }) {
  await openCategory(browser, category);
  await openNote(browser, title);
  await press(browser, browser.$('button[title="Version history"]'));
  await press(browser, browser.$('button.history-create-btn'));
  await browser.$('.history-item').waitForDisplayed({ timeout: 15_000 });
  const before = await browser.execute(() => document.querySelector('.ProseMirror').innerText.trim());
  await browser.execute(() => {
    const editor = document.querySelector('.ProseMirror');
    editor.focus();
    const range = document.createRange();
    range.selectNodeContents(editor);
    range.collapse(false);
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);
  });
  await type(browser, browser.$('.ProseMirror'), ' Edit made after the version.');
  const items = await browser.$$('.history-item');
  await press(browser, items[0]);
  await press(browser, browser.$('button.history-restore-btn'));
  await browser.waitUntil(
    async () => (await browser.execute(() => document.querySelector('.ProseMirror').innerText.trim())) === before,
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
  await browser.waitUntil(
    async () => (await browser.execute(() => document.querySelector('.ProseMirror')?.innerText ?? '')).includes(expectedText),
    { timeout: 60_000, timeoutMsg: `clipped note does not contain "${expectedText}"` },
  );
  const title = await browser.execute(() => document.querySelector('.editor-title input').value);
  return { url, category, title };
}

// The file picker is a native dialog WebDriver cannot answer, so the file is handed to the
// editor's own hidden input the way the picker would.
export async function attachFile(browser, { name, content }) {
  const attached = await browser.execute((fileName, text) => {
    const input = document.querySelector('#insert-file-input');
    if (!input) return false;
    const transfer = new DataTransfer();
    transfer.items.add(new File([text], fileName, { type: 'text/plain' }));
    input.files = transfer.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
    return true;
  }, name, content);
  if (!attached) fail('editor has no file input');
  await browser.waitUntil(
    async () => (await browser.execute(() => document.querySelector('.ProseMirror')?.innerText ?? '')).includes(name),
    { timeout: 30_000, timeoutMsg: `attachment ${name} did not appear in the note` },
  );
  return { name };
}

// The export button opens a native save dialog, which WebDriver cannot answer. The harness calls
// the button's own command with the path the dialog would have returned.
export async function exportDiagnostics(browser, path) {
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
// the token leaves the machine's keyring. The controller checks Notion itself afterwards.
export async function notionPublish(browser, type, { token, page }) {
  await openSettingsTab(browser, 'Notion');
  await type(browser, browser.$('input[placeholder="ntn_…"]'), token);
  await pressText(browser, 'button.import-btn', 'Connect');
  await pressText(browser, 'button.import-btn', 'Choose a page');
  await browser.waitUntil(async () => (await byText(browser, 'button.option-btn', page)).length === 1, {
    timeout: 60_000, timeoutMsg: `Notion page "${page}" is not shared with the integration`,
  });
  await pressText(browser, 'button.option-btn', page);
  const toggle = browser.$('button[aria-label="Publish to Notion from this machine"]');
  await toggle.waitForExist({ timeout: 60_000 });
  if ((await toggle.getAttribute('aria-checked')) !== 'true') await press(browser, toggle);
  await pressText(browser, 'button.import-btn', 'Publish Now');
  const summary = await waitForText(
    browser, 'body',
    (value) => /Last published:/.test(value) && !/Publishing/.test(value),
    'Notion publish did not finish', 10 * 60_000,
  );
  const error = await browser.execute(() => document.querySelector('.import-result.error')?.innerText.trim() ?? null);
  if (error) fail(`Notion publish failed: ${error}`);
  await pressText(browser, 'button.import-btn', 'Disconnect');
  await browser.waitUntil(async () => (await byText(browser, 'button.import-btn', 'Connect')).length === 1, {
    timeout: 30_000, timeoutMsg: 'Notion did not disconnect',
  });
  await closeSettings(browser);
  return { page, summary: /Last published:[^\n]*(\n[^\n]*){0,2}/.exec(summary)?.[0] };
}
