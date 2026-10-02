import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const helper = fileURLToPath(new URL('../scripts/linux-test-package.sh', import.meta.url));
const rpmbuild = spawnSync('rpmbuild', ['--version']).status === 0;

// Builds a one-file package shaped like a Second Brain build, changed by `extra`.
function buildRpm(topdir, label, { name = 'second-brain', arch = 'x86_64', preamble = '', files = '/usr/bin/second-brain', install = '', sections = '' } = {}) {
  const spec = join(topdir, `${label}.spec`);
  writeFileSync(spec, `Name: ${name}
Version: 1
Release: 1
Summary: test
License: MIT
BuildArch: ${arch}
${preamble}
%global debug_package %{nil}
%global __os_install_post %{nil}
%description
test
%install
mkdir -p %{buildroot}/usr/bin
echo app > %{buildroot}/usr/bin/second-brain
${install}
%files
${files}
${sections}
`);
  const out = join(topdir, label);
  mkdirSync(out);
  const built = spawnSync('rpmbuild', ['-bb', '--quiet', '--define', `_topdir ${topdir}`, '--define', `_rpmdir ${out}`, spec], { encoding: 'utf8' });
  assert.equal(built.status, 0, built.stderr);
  const [archDir] = readdirSync(out);
  const [rpm] = readdirSync(join(out, archDir));
  return join(out, archDir, rpm);
}

function check(rpm) {
  return spawnSync('bash', ['-c', 'source "$1" && check_candidate "$2"', 'check', helper, rpm], { encoding: 'utf8' });
}

test('the test package helper installs only a plain Second Brain package', { skip: !rpmbuild && 'rpmbuild not installed' }, () => {
  const topdir = mkdtempSync(join(tmpdir(), 'sb-test-package-'));
  try {
    const good = buildRpm(topdir, 'good', {
      install: 'mkdir -p %{buildroot}/usr/share/icons/hicolor/256x256@2/apps\necho png > %{buildroot}/usr/share/icons/hicolor/256x256@2/apps/second-brain.png',
      files: '/usr/bin/second-brain\n/usr/share/icons/hicolor/256x256@2/apps/second-brain.png',
    });
    const accepted = check(good);
    assert.equal(accepted.status, 0, accepted.stderr);

    const rejected = {
      name: [{ name: 'second-brain-other' }, /not a second-brain x86_64 package/],
      arch: [{ arch: 'noarch' }, /not a second-brain x86_64 package/],
      post: [{ sections: '%post\ntrue' }, /scriptlets or triggers/],
      luaPretrans: [{ sections: '%pretrans -p <lua>\nprint("x")' }, /scriptlets or triggers/],
      fileTrigger: [{ sections: '%filetriggerin -- /usr/lib\ntrue' }, /scriptlets or triggers/],
      obsoletes: [{ preamble: 'Obsoletes: sudo' }, /obsoletes other packages/],
      etc: [{ install: 'mkdir -p %{buildroot}/etc/sudoers.d\necho x > %{buildroot}/etc/sudoers.d/x', files: '/usr/bin/second-brain\n/etc/sudoers.d/x' }, /outside the Second Brain paths/],
      setuid: [{ files: '%attr(4755,root,root) /usr/bin/second-brain' }, /not a plain root-owned file/],
      owner: [{ files: '%attr(0755,nobody,nobody) /usr/bin/second-brain' }, /not a plain root-owned file/],
      caps: [{ files: '%caps(cap_net_raw=ep) /usr/bin/second-brain' }, /not a plain root-owned file/],
      directory: [{ files: '/usr/bin/second-brain\n%dir /usr/share/second-brain', install: 'mkdir -p %{buildroot}/usr/share/second-brain' }, /not a plain root-owned file/],
    };
    for (const [label, [options, reason]] of Object.entries(rejected)) {
      const result = check(buildRpm(topdir, label, options));
      assert.equal(result.status, 1, `${label} was accepted`);
      assert.match(result.stderr, reason, label);
    }

    // rpm reads a file that is not a package as a manifest of paths to open.
    const manifest = join(topdir, 'manifest.rpm');
    writeFileSync(manifest, `${good}\n`);
    assert.match(check(manifest).stderr, /not an RPM package/);
  } finally {
    rmSync(topdir, { recursive: true, force: true });
  }
});

test('the test package helper refuses to run without sudo', () => {
  const result = spawnSync('bash', [helper, 'remove-test-install'], { encoding: 'utf8', env: { ...process.env, SUDO_USER: '' } });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /run through sudo/);
});
