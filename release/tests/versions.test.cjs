// 通过真实发行脚本验证二开迭代排序、历史 tag 选择与 Latest 防回退
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const path = require('node:path');
const { test } = require('node:test');
const fixtures = require('./fixtures/fork-versions.json');

const script = path.resolve(__dirname, '../../.github/scripts/publish-release.rb');
const lifecycle = path.resolve(__dirname, '../../backend/crates/gateway-plugin/runtime/plugin-api-lifecycle.rb');
function invoke(method, input) {
  return JSON.parse(execFileSync('ruby', ['-r', script, '-e', `
    input = JSON.parse(STDIN.read)
    begin
      result = case ARGV.first
      when 'compare'
        ReleaseMetadata.compare(input.fetch('target'), input.fetch('current'))
      when 'version'
        ReleaseMetadata.version(input.fetch('version'))
      when 'previous'
        ReleaseMetadata.previous_tag(input.fetch('version'), input.fetch('tags'))
      when 'promote'
        ReleasePublication.can_promote?(input.fetch('version'), input.fetch('current'))
      end
      puts JSON.generate('result' => result)
    rescue StandardError => error
      puts JSON.generate('error' => error.message)
    end
  `, method], { input: JSON.stringify(input), encoding: 'utf8' }));
}

test('兼容窗口按二开正式发行计数，同版本重试不重复消耗', () => {
  const result = JSON.parse(execFileSync('ruby', ['-r', lifecycle, '-e', `
    host = {'capabilities' => [{'capability' => 'middleware', 'versions' => [5, 6]}]}
    entries = [{'capability' => 'middleware', 'version' => 5, 'replacement_version' => 6,
      'remaining_releases' => 3, 'introduced_in' => nil, 'last_counted_in' => nil}]
    first = PluginApiLifecycle.advance(entries, host, '3.33.0+lqy007700.v1')
    retry_first = PluginApiLifecycle.advance(first, host, '3.33.0+lqy007700.v1')
    second = PluginApiLifecycle.advance(first, host, '3.33.0+lqy007700.v2')
    beta = PluginApiLifecycle.advance(second, host, '3.33.0-beta.1+lqy007700.v3')
    ordinary = PluginApiLifecycle.advance(entries, host, '3.33.0+ordinary-a')
    ordinary_retry = PluginApiLifecycle.advance(ordinary, host, '3.33.0+ordinary-b')
    puts JSON.generate([first, retry_first, second, beta, ordinary, ordinary_retry])
  `], { encoding: 'utf8' }));
  const [first, retry, second, beta, ordinary, ordinaryRetry] = result.map(entries => entries[0]);
  assert.equal(first.introduced_in, '3.33.0+lqy007700.v1');
  assert.equal(first.remaining_releases, 3);
  assert.deepEqual(first, retry);
  assert.equal(second.last_counted_in, '3.33.0+lqy007700.v2');
  assert.equal(second.remaining_releases, 2);
  assert.deepEqual(second, beta);
  assert.deepEqual(ordinary, ordinaryRetry);
});

for (const fixture of fixtures.comparisons) {
  test(`版本排序 ${fixture.current} -> ${fixture.target}`, () => {
    assert.deepEqual(invoke('compare', fixture), { result: fixture.order });
  });
}

for (const version of fixtures.invalid) {
  test(`拒绝非法迭代号 ${version}`, () => {
    assert.match(invoke('version', { version }).error, /Invalid fork revision/);
  });
}

test('前一 tag 优先选择同基线最高迭代并忽略非法候选', () => {
  assert.deepEqual(invoke('previous', {
    version: '3.33.0+lqy007700.v10',
    tags: ['v3.32.0+lqy007700.v999', 'v3.33.0', 'v3.33.0+lqy007700', 'v3.33.0+lqy007700.v2', 'v3.33.0+lqy007700.v9', 'v3.33.0+lqy007700.v01', 'v3.33.0+lqy007700.v11'],
  }), { result: 'v3.33.0+lqy007700.v9' });
});

test('混合发行标识不能遮住前一基线的最高二开迭代', () => {
  assert.deepEqual(invoke('previous', {
    version: '3.34.0+lqy007700.v1',
    tags: ['v3.33.0+other', 'v3.33.0+lqy007700.v2', 'v3.33.0+lqy007700.v10'],
  }), { result: 'v3.33.0+lqy007700.v10' });
});

for (const [version, current, allowed] of [
  ['3.33.0+lqy007700.v2', ['v3.33.0+lqy007700.v1'], true],
  ['3.33.0+lqy007700.v2', ['v3.33.0+lqy007700.v2'], true],
  ['3.33.0+lqy007700.v2', ['v3.33.0+lqy007700.v10'], false],
  ['3.33.0+lqy007700.v2', ['v3.33.0+lqy007700.v1', '3.33.0+lqy007700.v10'], false],
  ['3.34.0+lqy007700.v1', ['v3.33.0+lqy007700.v99'], true],
  ['3.33.0-beta.1+lqy007700.v2', ['3.32.0+lqy007700.v1'], false],
  ['3.33.0+lqy007700.v2', ['3.33.0+other'], false],
]) {
  test(`Latest 提升 ${current.join(',')} -> ${version}`, () => {
    assert.deepEqual(invoke('promote', { version, current }), { result: allowed });
  });
}
