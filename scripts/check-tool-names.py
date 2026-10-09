"""Query the same shims as check.sh so overrides select the candidate binaries."""
import json
import os
import selectors
import subprocess
import tempfile
import time
from pathlib import Path


def tool_names(command, env):
    with tempfile.TemporaryFile() as stderr:
        try:
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=stderr, env=env, bufsize=0)
            try:
                with selectors.DefaultSelector() as selector:
                    selector.register(process.stdout, selectors.EVENT_READ)
                    pending = bytearray()

                    def request(identifier, method, params):
                        process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=identifier,
                                                           method=method, params=params)).encode() + b"\n")
                        process.stdin.flush()
                        deadline = time.monotonic() + 30
                        while time.monotonic() < deadline:
                            if b"\n" not in pending:
                                if not selector.select(max(0, deadline - time.monotonic())):
                                    break
                                chunk = os.read(process.stdout.fileno(), 65536)
                                if not chunk:
                                    raise RuntimeError("server exited")
                                pending.extend(chunk)
                                continue
                            line, _, rest = pending.partition(b"\n")
                            pending[:] = rest
                            response = json.loads(line)
                            if response.get("id") == identifier:
                                if "error" in response:
                                    raise RuntimeError(str(response["error"]))
                                return response["result"]
                        raise RuntimeError(f"{method} timed out")

                    request(1, "initialize", dict(protocolVersion="2024-11-05", capabilities={},
                                                  clientInfo=dict(name="bottega-check", version="1")))
                    process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")).encode() + b"\n")
                    process.stdin.flush()
                    names = set()
                    cursors = set()
                    params = {}
                    for page in range(50):
                        result = request(page + 2, "tools/list", params)
                        for tool in result["tools"]:
                            name = tool["name"]
                            if name in names:
                                raise RuntimeError(f"duplicate tool name {name!r} within server")
                            names.add(name)
                        cursor = result.get("nextCursor")
                        if not cursor:
                            return names
                        if cursor in cursors:
                            raise RuntimeError(f"repeated tools/list cursor {cursor!r}")
                        cursors.add(cursor)
                        params = dict(cursor=cursor)
                    raise RuntimeError("tools/list exceeded 50 pages")
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                for stream in (process.stdin, process.stdout):
                    stream.close()
        except Exception as error:
            # Reap first so startup and broken-pipe failures include final diagnostics.
            stderr.seek(0, os.SEEK_END)
            stderr.seek(max(0, stderr.tell() - 4096))
            tail = stderr.read().decode(errors="replace")
            raise RuntimeError(f"{command}: {error}\nServer stderr tail:\n{tail}") from error


def main():
    with tempfile.TemporaryDirectory(prefix="bottega-tool-check-") as scratch:
        env = dict(os.environ, TROVADOR_HOME=str(Path(scratch) / ".trovador"))
        # Metadata discovery starts no workers; inherited launch settings must
        # not prevent checking the server from an agent development shell.
        for suffix in ("CHILD", "DEFAULT_BACKEND", "MAX_CONCURRENCY"):
            for prefix in ("TOSI", "TASK_RUNNERS"):
                env.pop(f"{prefix}_{suffix}", None)
        owners = {}
        for server, args in (("trovador", ["serve"]), ("tosi", ["--stdio"])):
            names = tool_names([f"plugins/{server}/bin/{server}", *args], env)
            for name in sorted(names):
                if name in owners:
                    raise RuntimeError(f"tool {name!r} appears in both {owners[name]} and {server}")
                owners[name] = server
            print(f"tool names: {server}: {len(names)} unique")
        print(f"tool names: {len(owners)} unique across servers")


if __name__ == "__main__":
    main()
