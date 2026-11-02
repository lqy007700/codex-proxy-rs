// 校验容器缓存归档完整性，不依赖 Docker 或真实编译产物
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const path = require('node:path');
const test = require('node:test');

test('Cargo snapshots reject corrupt archives without extracting build artifacts', () => {
  const script = path.resolve(__dirname, '../../.github/actions/cargo-cache/cargo-cache.sh');
  const result = spawnSync('python3', ['-', script], {
    encoding: 'utf8',
    input: String.raw`
import io
import pathlib
import subprocess
import sys
import tarfile
import tempfile

source = pathlib.Path(sys.argv[1]).read_text()
validator = source.split("<<'PY'\n", 1)[1].split('\nPY\n', 1)[0]

with tempfile.TemporaryDirectory(prefix='cpr-cache-test-') as directory:
    root = pathlib.Path(directory)

    def archive(name, member):
        with tarfile.open(root / name, 'w:gz') as output:
            if member:
                entry = tarfile.TarInfo(member)
                entry.size = 8
                output.addfile(entry, io.BytesIO(b'contents'))

    def seed():
        archive('registry.tar.gz', 'cache/index/example.crate')
        archive('target.tar.gz', 'amd64/release/deps/libexample.rlib')
        archive('git.tar.gz', None)

    def check(expected, label):
        completed = subprocess.run([sys.executable, '-c', validator, str(root), 'amd64'], capture_output=True)
        assert completed.returncode == expected, (label, completed.returncode, completed.stderr.decode())
        assert all(path.suffix == '.gz' for path in root.iterdir()), 'validator extracted artifacts'

    check(1, 'cache miss')
    seed()
    check(0, 'valid snapshot with no git dependencies')
    archive('target.tar.gz', 'arm64/release/deps/libexample.rlib')
    check(1, 'wrong architecture')
    for name in ['registry.tar.gz', 'target.tar.gz', 'git.tar.gz']:
        seed()
        file = root / name
        file.write_bytes(file.read_bytes()[:-8])
        check(1, name + ' truncated gzip trailer')
        seed()
        data = bytearray(file.read_bytes())
        data[-8] ^= 255
        file.write_bytes(data)
        check(1, name + ' invalid CRC')
        seed()
        file.write_bytes(b'not a gzip archive')
        check(1, name + ' invalid header')
    seed()
    archive('registry.tar.gz', None)
    check(1, 'missing downloaded dependencies')
`,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout || String(result.error));
});
