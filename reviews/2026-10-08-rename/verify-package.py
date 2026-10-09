"""Bounded, fake-provider check of effort through a fresh packaged MCP server."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("smoke", ROOT / "scripts/smoke.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
binary = Path(sys.argv[1]).resolve()
evidence = {}
with tempfile.TemporaryDirectory(prefix="effort-package-check-") as tmp:
    directory = Path(tmp).resolve()
    env = dict(os.environ)
    for backend in ("claude", "codex"):
        env["TOSI_" + backend.upper() + "_BIN"] = smoke.fake_cli(directory, backend)
    client = smoke.Client(binary, env, directory)
    try:
        tools = client.rpc("tools/list")["tools"]
        spawn = next(t for t in tools if t["name"] == "spawn_task")
        effort_schema = spawn["inputSchema"]["properties"]["effort"]
        effort_values = spawn["inputSchema"]["$defs"]["Effort"]["enum"]
        assert effort_values == ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"]
        assert "effort" not in spawn["inputSchema"].get("required", [])
        evidence["effort_schema"] = effort_schema
        evidence["effort_enum"] = effort_values
        resources = client.rpc("resources/list")["resources"]
        assert len(resources) == 5
        for resource in resources:
            relative = resource["uri"].removeprefix("skill://tosi/")
            canonical = (ROOT / "resources/skills" / relative).read_text()
            packaged = (binary.parent.parent / "skills" / relative).read_text()
            contents = client.rpc("resources/read", {"uri": resource["uri"]})["contents"]
            assert len(contents) == 1 and contents[0]["text"] == canonical == packaged
        evidence["canonical_resource_parity"] = 5
        schema = {"type": "object", "properties": {"answer": {"type": "string"}},
                  "required": ["answer"], "additionalProperties": False}
        outcomes = {}
        for backend in ("claude", "codex"):
            task = client.tool("spawn_task", {
                "backend": backend, "cwd": str(directory), "task": "Fixture read",
                "effort": "low", "output_schema": schema, "timeout_secs": 10,
            })
            result = client.collect([task["task_id"]])[task["task_id"]]
            assert result["status"] == "completed", result
            expected = {"backend": backend, "model": None, "effort": "low"}
            assert result["selection"] == {"requested": expected, "selected": expected,
                                           "reported": {"model": None, "effort": None}}
            assert result["structured_result"] == {"answer": "ORBIT-742"}
            follow = client.tool("continue_task", {"task_id": task["task_id"], "message": "Fixture resume"})
            completed = client.collect([follow["task_id"]])[follow["task_id"]]
            assert completed["status"] == "completed", completed
            assert completed["effort"] == "low" and completed["selection"] == result["selection"]
            assert completed["structured_result"] == result["structured_result"]
            assert completed["provider_session_id"] == result["provider_session_id"]
            outcomes[backend] = "low effort, schema, selection and continuation passed"
        evidence["fake_provider_outcomes"] = outcomes
    finally:
        client.close()
print(json.dumps(evidence, indent=2))
