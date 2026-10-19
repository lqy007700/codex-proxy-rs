// 验证真实单平台镜像形状、多平台一致性及 latest 防回退边界
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const path = require('node:path');
const { test } = require('node:test');

const script = path.resolve(__dirname, '../../.github/scripts/publish-release.rb');
const image = version => ({
  created: '2026-10-16T09:59:50.80488023Z',
  architecture: 'amd64',
  os: 'linux',
  config: { Labels: { 'org.opencontainers.image.version': version } },
  rootfs: { type: 'layers', diff_ids: [] },
  history: [],
});

function invoke(method, data) {
  return JSON.parse(execFileSync('ruby', ['-r', script, '-e', `
    input = JSON.parse(STDIN.read)
    begin
      if ARGV.first == 'latest_image'
        Open3.define_singleton_method(:capture3) do |*command|
          raise 'Unexpected command' unless command == ['docker', 'buildx', 'imagetools', 'inspect', 'test/image:latest', '--format', '{{json .Image}}']
          [input.fetch('output'), input.fetch('error', ''), Struct.new(:success?).new(input.fetch('success', true))]
        end
        result = ReleasePublication.latest_image('test/image')
      else
        result = ReleasePublication.can_promote?(input.fetch('version'), input.fetch('current'))
      end
      puts JSON.generate('result' => result)
    rescue StandardError => error
      puts JSON.generate('error' => error.message)
    end
  `, method], { input: JSON.stringify(data), encoding: 'utf8' }));
}

function inspect(config) {
  return invoke('latest_image', { output: JSON.stringify(config) });
}

test('单平台镜像包含 created 等元数据时仍读取正确版本', () => {
  assert.deepEqual(inspect(image('3.28.0+lqy007700')), { result: '3.28.0+lqy007700' });
});

test('多平台镜像版本一致时允许读取', () => {
  assert.deepEqual(inspect({ 'linux/amd64': image('3.29.3+fork'), 'linux/arm64': image('3.29.3+fork') }), { result: '3.29.3+fork' });
});

test('多平台版本不一致时阻止提升 latest', () => {
  assert.match(inspect({ 'linux/amd64': image('3.29.3'), 'linux/arm64': image('3.28.0') }).error, /one consistent version/);
});

for (const [label, config] of [
  ['空集合', {}],
  ['数组', []],
  ['空值', null],
  ['缺失版本', { config: { Labels: {} } }],
  ['非法版本', image('not-a-version')],
]) {
  test(`拒绝${label}，不把无法识别当作首次发布`, () => {
    assert.ok(inspect(config).error);
  });
}

test('只有明确缺失标签才作为首次发布', () => {
  assert.deepEqual(invoke('latest_image', { output: '', error: 'test/image:latest: not found', success: false }), { result: null });
  for (const error of ['unauthorized', 'connection timeout']) {
    assert.match(invoke('latest_image', { output: '', error, success: false }).error, /Cannot read latest image/);
  }
});

for (const [label, version, current, expected] of [
  ['新正式版', '3.29.3+fork', ['v3.28.0+fork', '3.28.0+fork'], true],
  ['同版本失败重试', '3.29.3+fork', ['v3.29.3+fork', '3.28.0+fork'], true],
  ['GitHub 已有新版', '3.29.3+fork', ['v3.30.0+fork', '3.28.0+fork'], false],
  ['镜像已有新版', '3.29.3+fork', ['v3.28.0+fork', '3.30.0+fork'], false],
  ['同优先级不同发行身份', '3.29.3+fork', ['v3.29.3+other', '3.28.0+fork'], false],
  ['预发行', '3.30.0-beta.1+fork', ['v3.29.3+fork', '3.29.3+fork'], false],
]) {
  test(`${label}的 latest 提升规则`, () => {
    assert.deepEqual(invoke('can_promote?', { version, current }), { result: expected });
  });
}
