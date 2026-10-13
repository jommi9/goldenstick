"""Synthetic privacy fixtures only. Never put a real credential in these tests."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('check-private-files.py').resolve()
spec = importlib.util.spec_from_file_location('private_files', SCRIPT)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class Rules(unittest.TestCase):
    def test_private_paths(self):
        for path in ['outputs/report.json', 'nested/local-data/x', '.env.local',
                     'music/song.flac', 'PIONEER/export.pdb', 'credentials.json']:
            with self.subTest(path=path):
                self.assertTrue(guard.inspect_blob(path, b'{}', {}))

    def test_secret_rules_withhold_values(self):
        for data in [b'ghp_' + b'x' * 30, b'ya29.' + b'x' * 30,
                     b'-----BEGIN ' + b'PRIVATE KEY-----',
                     json.dumps({'refresh_token': 'x' * 30}).encode()]:
            reasons = guard.inspect_blob('innocent.txt', data, {})
            self.assertTrue(reasons)
            self.assertNotIn(data.decode(), str(reasons))

    def test_home_paths_and_examples(self):
        self.assertTrue(guard.inspect_blob('note.md', b'/Users/' + b'personal/Music', {}))
        self.assertFalse(guard.inspect_blob('note.md', b'/Users/' + b'example/Music', {}))
        self.assertFalse(guard.inspect_blob('.env.example', b'CLIENT_ID=', {}))

    def test_runtime_manifest_and_real_report_are_private(self):
        for value in [{"kind": "real_read_only"}, {"source": "/Volumes/PRIVATE", "files": [], "revision": "abc"}]:
            self.assertTrue(guard.inspect_blob('renamed.json', json.dumps(value).encode(), {}))

    def test_windows_home_paths(self):
        path = 'C:' + chr(92) + 'Users' + chr(92) + 'personal' + chr(92) + 'Music'
        self.assertTrue(guard.inspect_blob('note.txt', path.encode(), {}))
        self.assertTrue(guard.inspect_blob('note.json', json.dumps(path).encode(), {}))

    def test_redacted_diagnostics_require_exact_reviewed_hash(self):
        path = 'crates/boothready-platform/tests/fixtures/example.json'
        data = b'{"devices": [], "captures": []}'
        self.assertTrue(guard.inspect_blob(path, data, {}))
        approved = {path: hashlib.sha256(data).hexdigest()}
        self.assertFalse(guard.inspect_blob(path, data, approved))
        self.assertTrue(guard.inspect_blob(path, data + b' ', approved))


class IndexChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.git('init', '-q')

    def tearDown(self):
        self.temp.cleanup()

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.root, check=True, capture_output=True)

    def run_guard(self, *args):
        return subprocess.run([os.sys.executable, str(SCRIPT), *args], cwd=self.root,
                              capture_output=True, text=True)

    def test_reads_staged_blob_even_when_working_file_is_clean(self):
        secret = 'ghp_' + 'x' * 30
        (self.root / 'note.txt').write_text(secret)
        self.git('add', 'note.txt')
        (self.root / 'note.txt').write_text('clean working copy')
        result = self.run_guard()
        self.assertEqual(result.returncode, 1)
        self.assertNotIn(secret, result.stderr)
        self.assertIn('GitHub credential', result.stderr)

    def test_newlines_in_filenames_do_not_add_output_lines(self):
        # Windows cannot create a filename containing a newline.
        if os.name == 'nt':
            self.skipTest('POSIX filename test')
        name = 'note\nprivate.txt'
        (self.root / name).write_text('ghp_' + 'x' * 30)
        self.git('add', '--', name)
        result = self.run_guard()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(result.stderr.splitlines()), 2)

    def test_all_mode_catches_unchanged_tracked_files(self):
        (self.root / 'note.txt').write_text('ghp_' + 'x' * 30)
        self.git('add', 'note.txt')
        self.git('-c', 'user.name=Example', '-c', 'user.email=example@example.invalid',
                 'commit', '-qm', 'Synthetic fixture')
        self.assertEqual(self.run_guard().returncode, 0)
        self.assertEqual(self.run_guard('--all').returncode, 1)


if __name__ == '__main__':
    unittest.main()
