// 通过真实发行脚本验证二开迭代排序、历史 tag 选择与 Latest 防回退
const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const path = require('node:path');
const { test } = require('node:test');
const fixtures = require('./fixtures/fork-versions.json');

const script = path.resolve(__dirname, '../../.github/scripts/publish-release.rb');
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
