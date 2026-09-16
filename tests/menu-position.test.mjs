import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(new URL('../src/lib/utils/menu-position.ts', import.meta.url), 'utf8');
const { code } = await transformWithEsbuild(source, 'menu-position.ts', { loader: 'ts', format: 'esm', target: 'esnext' });
const { clampMenuPosition, placeSubmenu } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);

test('clamps a context menu to every viewport edge', () => {
  const viewport = { width: 800, height: 600 };
  assert.deepEqual(clampMenuPosition(790, 590, 220, 300, viewport), { x: 572, y: 292 });
  assert.deepEqual(clampMenuPosition(-20, -10, 220, 300, viewport), { x: 8, y: 8 });
});

test('places a submenu on the available side and clamps it vertically', () => {
  const viewport = { width: 800, height: 600 };
  assert.deepEqual(placeSubmenu({ left: 700, right: 780, top: 500 }, 140, 180, viewport), { x: 558, y: 412 });
  assert.deepEqual(placeSubmenu({ left: 20, right: 100, top: 500 }, 140, 180, viewport), { x: 102, y: 412 });
});
