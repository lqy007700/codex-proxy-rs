// 验证发布恢复的来源信任、版本绑定和任务放行边界
const assert = require('node:assert/strict');
const { execFileSync, spawnSync } = require('node:child_process');
const { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

const root = path.resolve(__dirname, '../..');
const workflow = JSON.parse(execFileSync('ruby', ['-ryaml', '-rjson', '-e',
  'puts JSON.generate(YAML.load_file(ARGV[0]))',
  path.join(root, '.github/workflows/release.yml')], { encoding: 'utf8' }));
const steps = workflow.jobs['release-info'].steps;
const validate = new (Object.getPrototypeOf(async function () {}).constructor)(
  'github', 'context', 'core', 'process',
  steps.find(step => step.name === 'Validate recovery source').with.script,
);

function fixture() {
  return {
    sourceId: '101',
    source: { id: 101, workflow_id: 7, event: 'workflow_dispatch', status: 'completed', conclusion: 'failure' },
    current: { id: 102, workflow_id: 7 },
    jobs: [
      'release-info', 'frontend', 'quality / backend', 'quality / workflow-lint',
      'build-docker-binary (linux-amd64)', 'build-docker-image (linux/amd64)',
    ].map(name => ({ name, conclusion: 'success' })),
    artifacts: [
      'frontend-dist', 'plugin-release-assets', 'codex-proxy-rs-linux-amd64', 'image-digest-linux-amd64',
    ].map(name => ({ name, expired: false })),
  };
}

async function runValidation(data) {
  const api = {
    getWorkflowRun: async params => ({ data: params.run_id === 102 ? data.current : data.source }),
    listJobsForWorkflowRun: Symbol('jobs'),
    listWorkflowRunArtifacts: Symbol('artifacts'),
  };
  const github = {
    rest: { actions: api },
    paginate: async (method, params) => {
      assert.equal(params.run_id, 101);
      if (method === api.listJobsForWorkflowRun) {
        assert.equal(params.filter, 'latest');
        return data.jobs;
      }
      assert.equal(method, api.listWorkflowRunArtifacts);
      return data.artifacts;
    },
  };
  await validate(github, { repo: { owner: 'owner', repo: 'gateway' }, runId: 102 }, { info() {} }, {
    env: {
      SOURCE_RUN_ID: data.sourceId,
      DOCKER_MATRIX: JSON.stringify({ include: [{ os: 'linux', arch: 'amd64', dockerPlatform: 'linux/amd64' }] }),
    },
  });
}

test('打包失败但质量和镜像检查通过的原始运行可以复用', async () => {
  await runValidation(fixture());
});

for (const [label, mutate] of [
  ['无效运行编号', data => { data.sourceId = '../101'; }],
  ['当前运行', data => { data.source.id = 102; }],
  ['其他工作流', data => { data.source.workflow_id = 8; }],
  ['PR 运行', data => { data.source.event = 'pull_request'; }],
  ['仍在运行', data => { data.source.status = 'in_progress'; }],
  ['已取消', data => { data.source.conclusion = 'cancelled'; }],
  ['后端检查失败', data => { data.jobs.find(job => job.name === 'quality / backend').conclusion = 'failure'; }],
  ['恢复运行跳过构建', data => { data.jobs.find(job => job.name.startsWith('build-docker-binary')).conclusion = 'skipped'; }],
  ['镜像未验证', data => { data.jobs = data.jobs.filter(job => !job.name.startsWith('build-docker-image')); }],
  ['产物过期', data => { data.artifacts[0].expired = true; }],
  ['产物缺失', data => { data.artifacts.pop(); }],
  ['产物同名歧义', data => { data.artifacts.push({ ...data.artifacts[0] }); }],
]) {
  test(`拒绝${label}`, async () => {
    const data = fixture();
    mutate(data);
    await assert.rejects(runValidation(data));
  });
}

test('封口清单必须同时匹配版本和提交', () => {
  const directory = mkdtempSync(path.join(tmpdir(), 'cpr-recovery-'));
  try {
    mkdirSync(path.join(directory, 'reuse-manifest'));
    const manifest = { schema_version: 1, sealed: true, gateway_version: '1.2.3+fork', gateway_git_sha: 'a'.repeat(40) };
    const command = steps.find(step => step.name === 'Verify recovery version and commit').run;
    for (const patch of [{}, { gateway_version: '1.2.4+fork' }, { gateway_git_sha: 'b'.repeat(40) }, { sealed: false }]) {
      writeFileSync(path.join(directory, 'reuse-manifest/plugin-release-manifest.json'), JSON.stringify({ ...manifest, ...patch }));
      const result = spawnSync('bash', ['-c', command], {
        encoding: 'utf8',
        env: { ...process.env, RUNNER_TEMP: directory, RELEASE_VERSION: manifest.gateway_version, RELEASE_SHA: manifest.gateway_git_sha },
      });
      assert.equal(result.status, Object.keys(patch).length ? 1 : 0, result.stderr);
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

function allowed(job, reuse, results, cancelled = false) {
  const expression = workflow.jobs[job].if.replace(/^\$\{\{\s*|\s*\}\}$/g, '')
    .replace(/needs\.([\w-]+)\.result/g, (_, name) => JSON.stringify(results[name]));
  return new Function('inputs', 'cancelled', `return (${expression});`)(
    { reuse_run_id: reuse ? '101' : '' }, () => cancelled,
  );
}

test('新版本要求构建成功，恢复允许跳过构建，来源失败或取消均阻断打包', () => {
  const normal = { 'release-info': 'success', frontend: 'success', 'build-docker-binary': 'success' };
  assert.equal(allowed('package-assets', false, normal), true);
  assert.equal(allowed('package-assets', false, { ...normal, frontend: 'failure' }), false);
  const recovery = { ...normal, frontend: 'skipped', 'build-docker-binary': 'skipped' };
  assert.equal(allowed('package-assets', true, recovery), true);
  assert.equal(allowed('package-assets', true, { ...recovery, 'release-info': 'failure' }), false);
  assert.equal(allowed('package-assets', true, recovery, true), false);
});

test('发布需要本轮质量检查和打包成功，恢复模式只接受约定的镜像跳过', () => {
  const results = { 'release-info': 'success', quality: 'success', 'package-assets': 'success', 'build-docker-image': 'success' };
  assert.equal(allowed('publish', false, results), true);
  assert.equal(allowed('publish', false, { ...results, 'build-docker-image': 'skipped' }), false);
  const recovery = { ...results, 'build-docker-image': 'skipped' };
  assert.equal(allowed('publish', true, recovery), true);
  for (const name of ['release-info', 'quality', 'package-assets', 'build-docker-image']) {
    assert.equal(allowed('publish', true, { ...recovery, [name]: 'failure' }), false);
  }
  assert.equal(allowed('publish', true, recovery, true), false);
});

test('恢复使用工作流固定提交的发布工具，保留原发行版本与说明', () => {
  const step = workflow.jobs.publish.steps.find(step => step.name === 'Load publication tooling for recovery');
  assert.equal(step.if, "inputs.reuse_run_id != ''");
  assert.equal(step.env.PUBLICATION_TOOLING_SHA, '${{ github.sha }}');
  const directory = mkdtempSync(path.join(tmpdir(), 'cpr-publication-tooling-'));
  const origin = path.join(directory, 'origin');
  const checkout = path.join(directory, 'checkout');
  mkdirSync(origin);
  const git = (...args) => execFileSync('git', args, { cwd: origin, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  try {
    git('init');
    git('config', 'user.name', 'Release Test');
    git('config', 'user.email', 'release-test@example.invalid');
    git('config', 'core.hooksPath', '/dev/null');
    mkdirSync(path.join(origin, '.github/scripts'), { recursive: true });
    mkdirSync(path.join(origin, 'release'));
    for (const name of ['publish-release.rb', 'release-metadata.rb'])
      writeFileSync(path.join(origin, '.github/scripts', name), 'old tooling\n');
    writeFileSync(path.join(origin, 'release/version.yaml'), 'version: "3.29.3+fork"\n');
    writeFileSync(path.join(origin, 'release/notes.md'), '# v3.29.3+fork\n');
    git('add', '.');
    git('commit', '-m', 'release fixture');
    const release = git('rev-parse', 'HEAD');
    for (const name of ['publish-release.rb', 'release-metadata.rb'])
      writeFileSync(path.join(origin, '.github/scripts', name), 'fixed tooling\n');
    writeFileSync(path.join(origin, 'release/version.yaml'), 'version: "3.30.0+fork"\n');
    writeFileSync(path.join(origin, 'release/notes.md'), '# v3.30.0+fork\n');
    git('add', '.');
    git('commit', '-m', 'future tooling fixture');
    const tooling = git('rev-parse', 'HEAD');
    execFileSync('git', ['clone', origin, checkout], { stdio: 'pipe' });
    execFileSync('git', ['checkout', '--detach', release], { cwd: checkout, stdio: 'pipe' });
    execFileSync('bash', ['-e', '-c', step.run], {
      cwd: checkout, stdio: 'pipe', env: { ...process.env, PUBLICATION_TOOLING_SHA: tooling },
    });
    for (const name of ['publish-release.rb', 'release-metadata.rb'])
      assert.equal(readFileSync(path.join(checkout, '.github/scripts', name), 'utf8'), 'fixed tooling\n');
    assert.equal(readFileSync(path.join(checkout, 'release/version.yaml'), 'utf8'), 'version: "3.29.3+fork"\n');
    assert.equal(readFileSync(path.join(checkout, 'release/notes.md'), 'utf8'), '# v3.29.3+fork\n');
    assert.equal(execFileSync('git', ['rev-parse', 'HEAD'], { cwd: checkout, encoding: 'utf8' }).trim(), release);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
