import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(new URL('../src/lib/utils/repair-banner.ts', import.meta.url), 'utf8');
const { code } = await transformWithEsbuild(source, 'repair-banner.ts', {
  loader: 'ts',
  format: 'esm',
  target: 'esnext'
});
const { repairBanner } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);

const recovered = {
  key: 'restore:recovered',
  stage: 'restore',
  message: 'A restore was interrupted before it finished. Your vault is back exactly as it was before the restore.',
  paths: []
};
const unowned = {
  key: 'restore:unowned',
  stage: 'restore',
  message: 'Restore folders were found next to your vault that no restore record accounts for.',
  paths: ['/vaults/.second-brain-restore-rollback-old', '/vaults/.second-brain-restore-stage-old']
};
const searchFailure = {
  key: 'search:index',
  stage: 'search',
  message: 'Search index update failed',
  paths: ['/vaults/vault']
};

test('no issues and no error shows no banner', () => {
  assert.equal(repairBanner({ issues: [] }, ''), null);
});

test('a recovered restore is a dismissible notice, not a repair', () => {
  assert.deepEqual(repairBanner({ issues: [recovered] }, ''), {
    kind: 'notice',
    key: 'restore:recovered',
    title: 'Restore interrupted',
    message: recovered.message,
    paths: []
  });
});

test('unowned restore folders are shown first, with every path, since they may be the only copy', () => {
  assert.deepEqual(repairBanner({ issues: [recovered, unowned] }, ''), {
    kind: 'notice',
    key: 'restore:unowned',
    title: 'Restore folders found',
    message: unowned.message,
    paths: ['/vaults/.second-brain-restore-rollback-old', '/vaults/.second-brain-restore-stage-old']
  });
});

test('a real repair outranks restore notices and counts only repairs', () => {
  assert.deepEqual(repairBanner({ issues: [recovered, searchFailure] }, ''), {
    kind: 'repair',
    key: null,
    title: 'Vault repair needed',
    message: '1 issue may leave filing or search results incomplete.',
    paths: ['/vaults/vault']
  });
});

test('a failed repair attempt keeps the repair banner with its error', () => {
  assert.deepEqual(repairBanner({ issues: [recovered] }, 'retry failed'), {
    kind: 'repair',
    key: null,
    title: 'Vault repair needed',
    message: 'retry failed',
    paths: []
  });
});
