import { createHash } from 'node:crypto';
import { access, chmod, copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const VERSION = '2.1.5';
const RELEASE = `https://github.com/syncthing/syncthing/releases/download/v${VERSION}`;
const TARGETS = {
  'linux:x64': {
    archive: `syncthing-linux-amd64-v${VERSION}.tar.gz`,
    sha256: '3d222b609f7ab2944e02748cb10488b4160d446b49e0eafc107ef2a525ab3486',
    triple: 'x86_64-unknown-linux-gnu',
    executable: 'syncthing',
  },
  'win32:x64': {
    archive: `syncthing-windows-amd64-v${VERSION}.zip`,
    sha256: '39571e4d0900c2a2cab14c0b170f49751340a869e49734ccc8079d9b98a7974b',
    triple: 'x86_64-pc-windows-msvc',
    executable: 'syncthing.exe',
  },
};

const target = TARGETS[`${process.platform}:${process.arch}`];
if (!target) {
  throw new Error(`Syncthing ${VERSION} is not pinned for ${process.platform}/${process.arch}`);
}

const outputDir = resolve('src-tauri/binaries');
const suffix = process.platform === 'win32' ? '.exe' : '';
const output = join(outputDir, `syncthing-${target.triple}${suffix}`);
const stamp = `${output}.version`;

try {
  if ((await readFile(stamp, 'utf8')).trim() === VERSION && await access(output).then(() => true, () => false)) {
    process.exit(0);
  }
} catch {}

const scratch = await mkdtemp(join(tmpdir(), 'second-brain-syncthing-'));
try {
  const archivePath = join(scratch, target.archive);
  const response = await fetch(`${RELEASE}/${target.archive}`);
  if (!response.ok) throw new Error(`download failed: HTTP ${response.status}`);
  const archive = Buffer.from(await response.arrayBuffer());
  const actual = createHash('sha256').update(archive).digest('hex');
  if (actual !== target.sha256) {
    throw new Error(`checksum mismatch for ${target.archive}: ${actual}`);
  }
  await writeFile(archivePath, archive);

  const unpacked = join(scratch, 'unpacked');
  await mkdir(unpacked);
  let extraction;
  if (process.platform === 'win32') {
    const extractor = join(scratch, 'extract.ps1');
    await writeFile(
      extractor,
      'param([string]$Archive, [string]$Destination)\nExpand-Archive -LiteralPath $Archive -DestinationPath $Destination\n',
    );
    extraction = spawnSync(
      'powershell.exe',
      ['-NoProfile', '-File', extractor, archivePath, unpacked],
      { stdio: 'inherit' },
    );
  } else {
    extraction = spawnSync('tar', ['-xzf', archivePath, '-C', unpacked], { stdio: 'inherit' });
  }
  if (extraction.status !== 0) throw new Error(`could not extract ${target.archive}`);

  const root = join(unpacked, `syncthing-${process.platform === 'win32' ? 'windows' : 'linux'}-amd64-v${VERSION}`);
  await mkdir(outputDir, { recursive: true });
  await copyFile(join(root, target.executable), output);
  if (process.platform !== 'win32') await chmod(output, 0o755);
  await writeFile(stamp, `${VERSION}\n`);
  console.log(`Prepared Syncthing ${VERSION}: ${basename(output)}`);
} finally {
  await rm(scratch, { recursive: true, force: true });
}
