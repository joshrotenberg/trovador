#!/usr/bin/env python3
"""Exercise the real MCP stdio boundary; --live uses authenticated provider CLIs."""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time


class Client:
    def __init__(self, binary, env, directory):
        self.log = open(directory / "server.log", "w+")
        self.process = subprocess.Popen(
            [str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=self.log, text=True, bufsize=1, env=env,
        )
        self.lines = queue.Queue()
        self.sequence = 0

        def read():
            for line in self.process.stdout:
                self.lines.put(line)
            self.lines.put(None)

        threading.Thread(target=read, daemon=True).start()
        self.initialization = self.rpc("initialize", {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "tosi-smoke", "version": "0.1.0"},
        })
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def rpc(self, method, params=None):
        self.sequence += 1
        request_id = self.sequence
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params or {}})
        deadline = time.monotonic() + 65
        while True:
            line = self.lines.get(timeout=max(0.01, deadline - time.monotonic()))
            if line is None:
                self.log.flush()
                self.log.seek(0)
                raise RuntimeError("server exited: " + self.log.read()[-4000:])
            response = json.loads(line)
            if response.get("id") != request_id:
                continue
            if "error" in response:
                raise RuntimeError(response["error"])
            return response["result"]

    def tool(self, name, arguments):
        result = self.rpc("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise RuntimeError(result)
        if "structuredContent" in result:
            return result["structuredContent"]
        return json.loads(next(c["text"] for c in result["content"] if c["type"] == "text"))

    def collect(self, ids, timeout=180):
        deadline = time.monotonic() + timeout
        pending = list(ids)
        results = {}
        while pending and time.monotonic() < deadline:
            result = self.tool("wait_tasks", {"task_ids": pending, "timeout_ms": 1000})
            for task in result["tasks"]:
                if task["status"] in ("completed", "failed", "cancelled"):
                    results[task["task_id"]] = task
                    pending.remove(task["task_id"])
        if pending:
            raise RuntimeError("tasks did not finish: " + repr(pending))
        return results

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            self.process.wait(timeout=15)
        self.log.close()
        assert self.process.returncode == 0, self.process.returncode


def fake_cli(directory, backend):
    binary = directory / ("fake-" + backend)
    binary.write_text("#!/usr/bin/env python3\n" + f"BACKEND = {backend!r}\n" + r'''
import json, os, sys, time
from pathlib import Path
prompt = sys.stdin.read()
Path("pid-" + BACKEND).write_text(str(os.getpid()))
if prompt == "BLOCK":
    time.sleep(60)
text = "ORBIT-742"
arguments = sys.argv[1:]

def option(name):
    return arguments[arguments.index(name) + 1] if name in arguments else None

if BACKEND == "claude":
    schema = option("--json-schema")
    if schema:
        json.loads(schema)
    result = {"type":"result", "subtype":"success", "result":"" if schema else text,
        "session_id":"session-claude", "is_error":False}
    if schema:
        result["structured_output"] = {"answer": text}
    print(json.dumps(result))
else:
    schema = option("--output-schema")
    if schema:
        json.loads(Path(schema).read_text())
    final = json.dumps({"answer": text}) if schema else text
    Path(option("--output-last-message")).write_text(final)
    for event in [
        {"type":"thread.started", "thread_id":"session-codex"},
        {"type":"item.completed", "item":{"id":"item_0", "type":"agent_message", "text":"Intermediate narration."}},
        {"type":"item.completed", "item":{"id":"item_1", "type":"agent_message", "text":final}},
        {"type":"turn.completed", "usage":{"input_tokens":1,"output_tokens":1}},
    ]:
        print(json.dumps(event))
''')
    binary.chmod(0o700)
    return str(binary)


def assert_dead(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return
    raise AssertionError(f"worker {pid} remains alive after cleanup")


def wait_pid(path, old_pid=None):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if path.exists() and path.read_text().strip():
            pid = int(path.read_text())
            if pid != old_pid:
                return pid
        time.sleep(0.02)
    raise RuntimeError("fake worker did not start")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[1] / "target/debug/tosi")
    parser.add_argument("--live", action="store_true")
    parser.add_argument("--edit-only", action="store_true", help="Test live workspace_write in a disposable directory instead of read/resume")
    parser.add_argument("--backend", choices=["claude", "codex", "both"], default="both")
    args = parser.parse_args()
    backends = ["claude", "codex"] if args.backend == "both" else [args.backend]
    with tempfile.TemporaryDirectory(prefix="tosi-smoke-") as tmp:
        directory = Path(tmp).resolve()
        (directory / "context.txt").write_text("ORBIT-742\n")
        env = dict(os.environ)
        if not args.live:
            env["TOSI_CLAUDE_BIN"] = fake_cli(directory, "claude")
            env["TOSI_CODEX_BIN"] = fake_cli(directory, "codex")
        client = Client(args.binary.resolve(), env, directory)
        try:
            assert client.initialization["serverInfo"]["name"] == "tosi"
            names = {tool["name"] for tool in client.rpc("tools/list")["tools"]}
            assert names == {"check_environment", "spawn_task", "wait_tasks", "continue_task", "cancel_task"}, names
            print("PASS MCP initialize and five-tool discovery", flush=True)
            skill_uri = "skill://tosi/delegate/SKILL.md"
            resources = client.rpc("resources/list")["resources"]
            assert skill_uri in {resource["uri"] for resource in resources}
            effort_uri = "skill://tosi/delegate/references/model-effort.md"
            assert len(resources) == 5, resources
            assert effort_uri in {resource["uri"] for resource in resources}
            assert skill_uri in client.initialization.get("instructions", "")
            content = client.rpc("resources/read", {"uri": skill_uri})["contents"]
            assert any("spawn_task" in item.get("text", "") for item in content)
            effort_content = client.rpc("resources/read", {"uri": effort_uri})["contents"]
            assert any("selection" in item.get("text", "") for item in effort_content)
            print("PASS standalone skill resource discovery and read", flush=True)
            if args.edit_only:
                if not args.live:
                    raise ValueError("--edit-only requires --live")
                edits = {}
                for backend in backends:
                    edits[backend] = client.tool("spawn_task", {
                        "backend": backend, "cwd": str(directory), "access": "workspace_write", "timeout_secs": 120,
                        "task": f"Create a file named proof-{backend}.txt in your working directory containing exactly ORBIT-742 followed by a newline. Use the file editing tools. Do not modify any other files or delegate. Reply done after creating it.",
                    })
                results = client.collect([task["task_id"] for task in edits.values()])
                for backend, task in edits.items():
                    result = results[task["task_id"]]
                    assert result["status"] == "completed", result
                    assert (directory / f"proof-{backend}.txt").read_text().strip() == "ORBIT-742"
                    print(f"PASS {backend} workspace_write creates requested file", flush=True)
                return
            tasks = {}
            for backend in backends:
                tasks[backend] = client.tool("spawn_task", {
                    "backend": backend, "cwd": str(directory), "access": "read_only", "timeout_secs": 120,
                    "task": "Read context.txt in your working directory. Remember its content for a follow-up. Reply with only that content. Do not edit anything or delegate.",
                })
            results = client.collect([task["task_id"] for task in tasks.values()])
            for backend, task in tasks.items():
                result = results[task["task_id"]]
                assert result["status"] == "completed", result
                assert "ORBIT-742" in result["result"], result
                if not args.live:
                    assert result["result"] == "ORBIT-742", result
                assert result["provider_session_id"], result
                print(f"PASS {backend} bounded read-only run", flush=True)
            followups = {}
            for backend, task in tasks.items():
                followups[backend] = client.tool("continue_task", {
                    "task_id": task["task_id"],
                    "message": "What was the content you remembered from the previous turn? Do not read files again. Reply with only that content.",
                })
            results = client.collect([task["task_id"] for task in followups.values()])
            for backend, task in followups.items():
                result = results[task["task_id"]]
                assert result["status"] == "completed", result
                assert "ORBIT-742" in result["result"], result
                assert task["task_id"] != tasks[backend]["task_id"]
                assert task["conversation_id"] == tasks[backend]["conversation_id"]
                print(f"PASS {backend} new process resumes prior conversation", flush=True)
            if not args.live:
                schema = {
                    "type": "object", "properties": {"answer": {"type": "string"}},
                    "required": ["answer"], "additionalProperties": False,
                }
                structured_tasks = {}
                for backend in backends:
                    structured_tasks[backend] = client.tool("spawn_task", {
                        "backend": backend, "cwd": str(directory),
                        "task": "Return the remembered content as answer.",
                        "output_schema": schema,
                    })
                results = client.collect([task["task_id"] for task in structured_tasks.values()])
                for backend, task in structured_tasks.items():
                    result = results[task["task_id"]]
                    assert result["status"] == "completed", result
                    assert result["structured_result"] == {"answer": "ORBIT-742"}, result
                    followup = client.tool("continue_task", {
                        "task_id": task["task_id"], "message": "Return the answer again.",
                    })
                    result = client.collect([followup["task_id"]])[followup["task_id"]]
                    assert result["status"] == "completed", result
                    assert result["structured_result"] == {"answer": "ORBIT-742"}, result
                    print(f"PASS {backend} structured output and schema-preserving continuation", flush=True)
                pid_path = directory / "pid-codex"
                old_pid = int(pid_path.read_text()) if pid_path.exists() else None
                blocked = client.tool("spawn_task", {"backend": "codex", "cwd": str(directory), "task": "BLOCK"})
                pid = wait_pid(pid_path, old_pid)
                cancelled = client.tool("cancel_task", {"task_id": blocked["task_id"]})
                assert cancelled["status"] == "cancelled", cancelled
                assert_dead(pid)
                print("PASS explicit cancellation reaps worker", flush=True)
                client.tool("spawn_task", {"backend": "codex", "cwd": str(directory), "task": "BLOCK"})
                pid = wait_pid(pid_path, pid)
                client.close()
                assert_dead(pid)
                print("PASS host disconnect reaps active worker", flush=True)
                return
        finally:
            if not client.log.closed:
                client.close()
        print("PASS live smoke complete", flush=True)


if __name__ == "__main__":
    main()
