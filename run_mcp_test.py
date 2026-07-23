import json
import subprocess
import os

# build ink-mcp-server
os.system("cargo build -p ink-mcp-server")

# Write a fake connection file
os.makedirs("/tmp/ink-mcp-test", exist_ok=True)
with open("/tmp/ink-mcp-test/ipc.json", "w") as f:
    json.dump({"port": 12345, "token": "test-token"}, f)

p = subprocess.Popen(
    ["./target/debug/ink-mcp-server", "--connect", "/tmp/ink-mcp-test/ipc.json"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True
)

init_req = json.dumps({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {"name": "test-client", "version": "1.0"}
    }
})

print(f"Sending: {init_req}")
p.stdin.write(init_req + "\n")
p.stdin.flush()

res = p.stdout.readline()
print(f"Response: {res}")

list_req = json.dumps({
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/list",
    "params": {}
})

print(f"Sending: {list_req}")
p.stdin.write(list_req + "\n")
p.stdin.flush()

res2 = p.stdout.readline()
print(f"Response: {res2}")

p.terminate()
