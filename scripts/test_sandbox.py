"""Sandbox isolation checks; never starts a GUI, core, or real display.

Run: python3 -m unittest discover -s scripts -p 'test_sandbox.py' -v
"""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("sandbox").resolve()


class SandboxIsolation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="sandbox-test-")
        self.base = Path(self.temp.name)
        self.root = self.base / "private"
        self.env = dict(os.environ, HERDR_APP_SANDBOX="browser-test",
                        HERDR_APP_SANDBOX_ROOT=str(self.root))

    def tearDown(self):
        self.temp.cleanup()

    def run_script(self, *args):
        return subprocess.run(["bash", str(SCRIPT), *args], env=self.env,
                              capture_output=True, text=True)

    def fake_tools(self):
        tools = self.base / "tools"
        tools.mkdir()
        # Fail at build, after preparing the private environment. No app opens.
        cargo = tools / "cargo"
        cargo.write_text("#!/bin/sh\nenv >\"$HERDR_APP_SANDBOX_ROOT/environment\"\nexit 17\n")
        cargo.chmod(0o700)
        self.env["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]

    def test_rejects_dangerous_roots(self):
        for root in ("/", str(Path.home()), "relative", str(self.base / ".." / "other")):
            self.env["HERDR_APP_SANDBOX_ROOT"] = root
            self.assertEqual(self.run_script("down").returncode, 2, root)

    def test_rejects_symlink_root(self):
        self.root.symlink_to(self.base, target_is_directory=True)
        self.assertEqual(self.run_script("down").returncode, 2)

    def test_rejects_name_with_path_or_shell_syntax(self):
        for name in ("../production", "x y", "x;id", ""):
            # Empty selects the default, so only nonempty invalid names fail.
            if name:
                self.env["HERDR_APP_SANDBOX"] = name
                self.assertEqual(self.run_script("up").returncode, 2)

    def test_never_claims_or_deletes_existing_unowned_directory(self):
        self.root.mkdir()
        keep = self.root / "user-work"
        keep.write_text("do not delete")
        for command in ("up", "restart", "down"):
            self.assertEqual(self.run_script(command).returncode, 1)
            self.assertEqual(keep.read_text(), "do not delete")
            self.assertFalse((self.root / ".sandbox-owner").exists())

    def test_private_environment_and_explicit_core_are_used(self):
        self.fake_tools()
        core = self.base / "feature-core"
        core.write_text("#!/bin/sh\nexit 0\n")
        core.chmod(0o700)
        self.env["HERDR_APP_SANDBOX_CORE"] = str(core)
        result = self.run_script("up")
        self.assertEqual(result.returncode, 17, result.stderr)
        env = dict(line.split("=", 1) for line in (self.root / "environment").read_text().splitlines() if "=" in line)
        for key, folder in (("HERDR_APP_HOME", "home"), ("XDG_STATE_HOME", "state"),
                            ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
                            ("XDG_DATA_HOME", "data"), ("XDG_RUNTIME_DIR", "runtime"),
                            ("TMPDIR", "tmp")):
            self.assertEqual(env[key], str(self.root / folder))
        self.assertEqual(env["HC_SESSION"], "herdr-app-browser-test")
        self.assertEqual((self.root / "runtime").stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.root / "bin" / "hc").resolve(), core)

    def test_missing_root_cleanup_never_invokes_core(self):
        self.assertEqual(self.run_script("down").returncode, 0)
        self.assertFalse(self.root.exists())

    def test_input_and_screenshot_never_use_inherited_real_display(self):
        self.env["DISPLAY"] = ":0"
        for command in (("click", "10", "10"), ("key", "Return"), ("shot",)):
            result = self.run_script(*command)
            self.assertEqual(result.returncode, 1)
            self.assertIn("no owned private display", result.stderr)

    def test_wrong_owner_refuses_cleanup(self):
        self.root.mkdir()
        (self.root / ".sandbox-owner").write_text("someone-else")
        self.assertEqual(self.run_script("down").returncode, 1)
        self.assertTrue(self.root.exists())

    def test_symlink_owner_marker_cannot_authorize_cleanup(self):
        self.root.mkdir()
        marker = self.base / "unrelated-marker"
        marker.write_text(f"{os.getuid()}:browser-test")
        (self.root / ".sandbox-owner").symlink_to(marker)
        self.assertEqual(self.run_script("down").returncode, 1)
        self.assertTrue(marker.exists())

    def test_invalid_core_fails_before_build_or_display(self):
        self.fake_tools()
        self.env["HERDR_APP_SANDBOX_CORE"] = "relative-core"
        self.assertEqual(self.run_script("up").returncode, 1)
        self.assertFalse((self.root / "environment").exists())
        self.assertFalse((self.root / "xpid").exists())

    def test_owned_cleanup_only_invokes_isolated_test_core(self):
        self.fake_tools()
        core = self.base / "feature-core"
        record = self.base / "core-call"
        core.write_text(f'#!/bin/sh\nprintf "%s\\n" "$HC_SESSION" "$XDG_STATE_HOME" "$*" >"{record}"\n')
        core.chmod(0o700)
        self.env["HERDR_APP_SANDBOX_CORE"] = str(core)
        self.assertEqual(self.run_script("up").returncode, 17)
        self.assertEqual(self.run_script("down").returncode, 0)
        self.assertEqual(record.read_text().splitlines(), ["herdr-app-browser-test", str(self.root / "state"), "server stop"])
        self.assertFalse(self.root.exists())


if __name__ == "__main__":
    unittest.main()
