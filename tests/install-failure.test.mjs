import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repo = new URL('../', import.meta.url);
const repoPath = fileURLToPath(repo);

test('pnpm install fails when the prepare svelte-kit sync fails', async () => {
  const packageJson = JSON.parse(await readFile(new URL('package.json', repo), 'utf8'));
  assert.equal(packageJson.scripts.prepare, 'svelte-kit sync');

  const fixture = await mkdtemp(join(tmpdir(), 'second-brain-install-failure-'));
  try {
    await writeFile(
      join(fixture, 'package.json'),
      JSON.stringify({ private: true, scripts: { prepare: packageJson.scripts.prepare } }),
    );
    await writeFile(
      join(fixture, 'svelte.config.js'),
      "throw new Error('INTENTIONAL_SYNC_FAILURE');\n",
    );

    const result = spawnSync('pnpm', ['install', '--offline'], {
      cwd: fixture,
      encoding: 'utf8',
      env: {
        ...process.env,
        PATH: `${join(repoPath, 'node_modules', '.bin')}${delimiter}${process.env.PATH ?? ''}`,
      },
    });

    assert.notEqual(result.status, 0, 'install must propagate prepare failure');
    assert.match(`${result.stdout}\n${result.stderr}`, /INTENTIONAL_SYNC_FAILURE/);
  } finally {
    await rm(fixture, { recursive: true, force: true });
  }
});
