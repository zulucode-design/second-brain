import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { transformWithEsbuild } from 'vite';

const source = await readFile(new URL('../src/lib/utils/graph-repulsion.ts', import.meta.url), 'utf8');
const { code } = await transformWithEsbuild(source, 'graph-repulsion.ts', { loader: 'ts', format: 'esm' });
const { applyRepulsion, DIRECT_LIMIT } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);

function cloud(count, seed) {
  let state = seed;
  const random = () => ((state = (state * 1664525 + 1013904223) % 4294967296) / 4294967296);
  return Array.from({ length: count }, () => ({ x: random() * 1200, y: random() * 1200, vx: 0, vy: 0 }));
}

function exact(bodies) {
  return bodies.map((a) => {
    let vx = 0, vy = 0;
    for (const b of bodies) {
      if (a === b) continue;
      const dx = b.x - a.x, dy = b.y - a.y, distSq = dx * dx + dy * dy;
      if (distSq > 360000) continue;
      const d = distSq || 1, dist = Math.sqrt(d), force = 1500 / d;
      vx -= (dx / dist) * force; vy -= (dy / dist) * force;
    }
    return { vx, vy };
  });
}

test('small graphs keep exact pairwise repulsion', () => {
  const bodies = cloud(DIRECT_LIMIT, 7);
  const expected = exact(bodies);
  applyRepulsion(bodies);
  bodies.forEach((b, i) => {
    assert.ok(Math.abs(b.vx - expected[i].vx) < 1e-9 && Math.abs(b.vy - expected[i].vy) < 1e-9);
  });
});

test('large graphs approximate the exact repulsion closely', () => {
  const bodies = cloud(3000, 11);
  const expected = exact(bodies);
  applyRepulsion(bodies);
  let error = 0, magnitude = 0;
  bodies.forEach((b, i) => {
    error += Math.hypot(b.vx - expected[i].vx, b.vy - expected[i].vy);
    magnitude += Math.hypot(expected[i].vx, expected[i].vy);
  });
  assert.ok(error / magnitude < 0.05, `relative error ${(error / magnitude).toFixed(3)}`);
});

test('coincident bodies do not recurse forever or produce NaN', () => {
  const bodies = Array.from({ length: DIRECT_LIMIT + 10 }, () => ({ x: 5, y: 5, vx: 0, vy: 0 }));
  applyRepulsion(bodies);
  assert.ok(bodies.every((b) => Number.isFinite(b.vx) && Number.isFinite(b.vy)));
});
