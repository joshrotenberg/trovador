"""Exercise discovery against bounded fake servers without provider access."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("check_tool_names", Path(__file__).with_name("check-tool-names.py"))
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)

SERVER = r'''
import json, sys
mode = sys.argv[1]
print("discovery diagnostic", file=sys.stderr, flush=True)
if mode == "exit":
    print("x" * 5000 + "stderr-tail-sentinel", file=sys.stderr, flush=True)
    sys.exit(1)
page = 0
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    result = {}
    if request["method"] == "tools/list":
        page += 1
        result = {"tools": [{"name": "tool-" + str(page)}]}
        if mode == "duplicate":
            result["tools"] *= 2
        elif mode == "duplicate-page":
            result["tools"] = [{"name": "same"}]
            result["nextCursor"] = str(page)
        elif mode == "repeated":
            result["nextCursor"] = "same"
        elif mode == "unbounded" or (mode == "paged" and page == 1):
            result["nextCursor"] = str(page)
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
'''


class DiscoveryTests(unittest.TestCase):
    def discover(self, mode):
        return check.tool_names([sys.executable, "-u", "-c", SERVER, mode], os.environ.copy())

    def test_paged_tools_are_collected(self):
        self.assertEqual(self.discover("paged"), {"tool-1", "tool-2"})

    def test_startup_failure_includes_bounded_stderr_tail(self):
        with self.assertRaises(RuntimeError) as error:
            self.discover("exit")
        self.assertIn("stderr-tail-sentinel", str(error.exception))
        tail = str(error.exception).split("Server stderr tail:\n", 1)[1]
        self.assertLessEqual(len(tail.encode()), 4096)

    def test_broken_pipe_includes_stderr(self):
        real_popen = subprocess.Popen

        def broken_popen(*args, **kwargs):
            process = real_popen(*args, **kwargs)
            process.wait(timeout=5)
            process.stdin.close()
            process.stdin = unittest.mock.Mock()
            process.stdin.write.side_effect = BrokenPipeError("test broken pipe")
            return process

        with patch.object(check.subprocess, "Popen", side_effect=broken_popen):
            with self.assertRaisesRegex(RuntimeError, "test broken pipe.*") as error:
                self.discover("exit")
        self.assertIn("stderr-tail-sentinel", str(error.exception))

    def test_duplicate_names_within_and_across_pages_fail(self):
        for mode in ("duplicate", "duplicate-page"):
            with self.subTest(mode=mode):
                with self.assertRaisesRegex(RuntimeError, "duplicate tool name") as error:
                    self.discover(mode)
                self.assertIn("discovery diagnostic", str(error.exception))

    def test_repeated_cursor_fails(self):
        with self.assertRaisesRegex(RuntimeError, "repeated tools/list cursor"):
            self.discover("repeated")

    def test_fifty_page_bound_fails(self):
        with self.assertRaisesRegex(RuntimeError, "exceeded 50 pages"):
            self.discover("unbounded")

    def test_child_environment_is_isolated(self):
        settings = {f"{prefix}_{suffix}": "invalid" for prefix in ("TOSI", "TASK_RUNNERS")
                    for suffix in ("CHILD", "DEFAULT_BACKEND", "MAX_CONCURRENCY")}

        def discover(command, env):
            self.assertTrue(all(key not in env for key in settings))
            self.assertTrue(Path(env["TROVADOR_HOME"]).parent.is_dir())
            return {Path(command[0]).name}

        with patch.dict(os.environ, settings), patch.object(check, "tool_names", side_effect=discover):
            check.main()


if __name__ == "__main__":
    unittest.main()
