// 通过真实打包脚本验证旧更新器所需的发行身份和最终归档校验和
const assert = require('node:assert/strict');
const { execFileSync, spawnSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, statSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

const script = path.resolve(__dirname, '../../.github/scripts/package-release.rb');
const version = '3.39.1+lqy007700.v11';
const gitSha = 'a'.repeat(40);
const manifestPath = 'codex-proxy-rs/plugins/official/plugin-release-manifest.json';

function fixture(t) {
  const directory = mkdtempSync(path.join(tmpdir(), 'cpr-package-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  for (const name of ['deploy', 'dist/frontend', 'dist/binaries/codex-proxy-rs-linux-amd64'])
    mkdirSync(path.join(directory, name), { recursive: true });
  writeFileSync(path.join(directory, 'deploy/config.example.yaml'), 'server: {}\n');
  writeFileSync(path.join(directory, 'deploy/compose.yaml'), 'image: ${CPR_IMAGE:-ghcr.io/example/gateway:latest}\n');
  writeFileSync(path.join(directory, 'dist/frontend/index.html'), '<html>package fixture</html>');
  writeFileSync(path.join(directory, 'dist/binaries/codex-proxy-rs-linux-amd64/codex-proxy-rs'), 'binary fixture');
  const env = {
    ...process.env,
    VERSION: version,
    GIT_SHA: gitSha,
    DOCKER_VERSION: version.replace('+', '-'),
    REGISTRY: 'ghcr.io',
    IMAGE_NAME: 'example/gateway',
    PLATFORM_MATRIX: JSON.stringify({ include: [{ os: 'linux', arch: 'amd64', binary: 'codex-proxy-rs', archive: 'tar.gz' }] }),
  };
  return {
    directory, env,
    archive: path.join(directory, `dist/release/codex-proxy-rs_${version}_linux_amd64.tar.gz`),
    package: () => execFileSync('ruby', [script, 'archives'], { cwd: directory, env, encoding: 'utf8' }),
  };
}

test('归档携带匹配目标版本和提交的只读兼容清单，保留程序与页面', t => {
  const data = fixture(t);
  data.package();
  const read = name => execFileSync('tar', ['-xOzf', data.archive, name], { encoding: 'utf8' });
  assert.deepEqual(JSON.parse(read(manifestPath)), {
    schema_version: 1,
    sealed: true,
    gateway_version: version,
    gateway_git_sha: gitSha,
    plugin_host: null,
    plugins: [],
  });
  assert.equal(statSync(path.join(data.directory, 'package/linux-amd64', manifestPath)).mode & 0o777, 0o444);
  assert.equal(read('codex-proxy-rs/codex-proxy-rs'), 'binary fixture');
  assert.equal(read('codex-proxy-rs/web/dist/index.html'), '<html>package fixture</html>');
  const entries = execFileSync('tar', ['-tzf', data.archive], { encoding: 'utf8' }).trim().split('\n');
  assert.deepEqual(entries.filter(name => name.includes('/plugins/') && !name.endsWith('/')), [manifestPath]);
  const checksum = createHash('sha256').update(readFileSync(data.archive)).digest('hex');
  assert.ok(readFileSync(path.join(data.directory, 'dist/release/checksums.txt'), 'utf8')
    .includes(`${checksum}  ${path.basename(data.archive)}\n`));
});

test('重复打包可替换上次生成的只读兼容清单', t => {
  const data = fixture(t);
  data.package();
  data.env.GIT_SHA = 'b'.repeat(40);
  data.package();
  const manifest = JSON.parse(execFileSync('tar', ['-xOzf', data.archive, manifestPath], { encoding: 'utf8' }));
  assert.equal(manifest.gateway_git_sha, data.env.GIT_SHA);
});

for (const [name, patch, error] of [
  ['缺失提交身份', { GIT_SHA: undefined }, /key not found: "GIT_SHA"/],
  ['非法提交身份', { GIT_SHA: 'unknown' }, /Invalid release Git SHA/],
  ['非法目标版本', { VERSION: 'not-a-version' }, /Invalid release version/],
]) {
  test(`打包拒绝${name}`, t => {
    const data = fixture(t);
    const result = spawnSync('ruby', [script, 'archives'], {
      cwd: data.directory, env: { ...data.env, ...patch }, encoding: 'utf8',
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, error);
  });
}
