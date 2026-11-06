// 验证共享二进制构建及依赖缓存合同，不访问 Docker 或执行真实编译
const assert = require('node:assert/strict');
const { execFileSync, spawnSync } = require('node:child_process');
const { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync, chmodSync } = require('node:fs');
const path = require('node:path');
const { tmpdir } = require('node:os');
const test = require('node:test');

const root = path.resolve(__dirname, '../..');
const action = JSON.parse(execFileSync('ruby', ['-ryaml', '-rjson', '-e',
  'puts JSON.generate(YAML.load_file(ARGV[0]))',
  path.join(root, '.github/actions/build-binary/action.yml')], { encoding: 'utf8' }));
const script = action.runs.steps.find(step => step.name === 'Build binary').run;

function build(image = '', fail = false) {
  const directory = mkdtempSync(path.join(tmpdir(), 'cpr-build-binary-'));
  mkdirSync(path.join(directory, 'backend'));
  mkdirSync(path.join(directory, 'bin'));
  const compiler = `#!/bin/sh
set -eu
printf '%s\\n' "$*" > "$TEST_ROOT/compiler-arguments"
test "\${FAIL_BUILD:-false}" != true
mkdir -p "$TEST_ROOT/backend/target/$BUILD_TARGET/release"
cat > "$TEST_ROOT/backend/target/$BUILD_TARGET/release/$BUILD_BINARY" <<'BINARY'
#!/bin/sh
printf '%s\\n' "$*" >> "$TEST_ROOT/smoke-arguments"
BINARY
chmod +x "$TEST_ROOT/backend/target/$BUILD_TARGET/release/$BUILD_BINARY"
`;
  for (const name of ['cargo', 'docker']) {
    const file = path.join(directory, 'bin', name);
    writeFileSync(file, compiler);
    chmodSync(file, 0o755);
  }
  const result = spawnSync('bash', ['-c', script], {
    cwd: path.join(directory, 'backend'),
    env: {
      ...process.env,
      PATH: `${path.join(directory, 'bin')}:${process.env.PATH}`,
      TEST_ROOT: directory,
      BUILD_TARGET: 'x86_64-unknown-linux-gnu',
      BUILD_PLATFORM: 'linux-amd64',
      BUILD_BINARY: 'codex-proxy-rs',
      RUST_IMAGE: image,
      FAIL_BUILD: String(fail),
      CPR_VERSION: '3.41.1+lqy007700.v1',
      CPR_GIT_SHA: 'a'.repeat(40),
      CPR_BUILD_TIME: '2026-11-06T00:00:00Z',
      CPR_BUILD_TYPE: 'release',
    },
    encoding: 'utf8',
  });
  return { directory, result };
}

for (const image of ['', 'rust:1.99.1-bookworm@sha256:fixture']) {
  test(`${image ? 'Docker' : 'Native'}构建保留目标、元数据并验证同一二进制`, () => {
    const { directory, result } = build(image);
    try {
      assert.equal(result.status, 0, result.stderr);
      const args = readFileSync(path.join(directory, 'compiler-arguments'), 'utf8');
      assert.match(args, /cargo build --release --locked --timings --target x86_64-unknown-linux-gnu --bin codex-proxy-rs|^build --release --locked --timings --target x86_64-unknown-linux-gnu --bin codex-proxy-rs/);
      if (image) {
        for (const field of ['CPR_VERSION', 'CPR_GIT_SHA', 'CPR_BUILD_TIME', 'CPR_BUILD_TYPE'])
          assert.ok(args.includes(`-e ${field}`));
        assert.ok(args.includes(image));
      }
      assert.equal(readFileSync(path.join(directory, 'smoke-arguments'), 'utf8'), '--help\n--version\n');
      assert.ok(existsSync(path.join(directory, 'dist/binaries/linux-amd64/codex-proxy-rs')));
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
}

test('构建失败不能复制未验证的发行二进制', () => {
  const { directory, result } = build('', true);
  try {
    assert.notEqual(result.status, 0);
    assert.ok(!existsSync(path.join(directory, 'dist/binaries/linux-amd64/codex-proxy-rs')));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('依赖缓存按目标和构建镜像隔离，PR不保存缓存', () => {
  const cache = action.runs.steps.find(step => step.name === 'Cache Rust dependencies').with;
  assert.equal(cache['shared-key'], 'release-${{ inputs.target }}-${{ inputs.rust-image }}');
  assert.equal(cache['cache-bin'], false);
  assert.equal(cache['save-if'], "${{ github.event_name != 'pull_request' }}");
});
