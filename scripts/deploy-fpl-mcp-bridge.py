"""Run on Batty through stdin; deploy a tested local gateway image with rollback."""
import datetime, http.client, json, os, subprocess, sys, tempfile, time
from pathlib import Path

root = Path("/home/ajmwagar/librechat")
compose_file = root / "docker-compose.override.yaml"
image = sys.argv[1]

def run(*args):
    return subprocess.check_output(args, cwd=root, text=True, stderr=subprocess.PIPE)

def inspect():
    return json.loads(run("docker", "inspect", "librechat-fpl-mcp-bridge-1"))[0]

def write(text):
    stat = compose_file.stat()
    fd, path = tempfile.mkstemp(dir=root, prefix=".mcp-bridge-")
    try:
        os.fchmod(fd, stat.st_mode & 0o777); os.fchown(fd, stat.st_uid, stat.st_gid)
        with os.fdopen(fd, "w") as stream:
            stream.write(text); stream.flush(); os.fsync(stream.fileno())
        os.replace(path, compose_file)
    finally:
        if os.path.exists(path): os.unlink(path)

def compose():
    run("docker", "compose", "-f", str(root / "docker-compose.yml"), "-f",
        str(compose_file), "up", "-d", "--no-deps", "--pull", "never", "fpl-mcp-bridge")

def probe(path):
    conn = http.client.HTTPConnection("127.0.0.1", 8792, timeout=5)
    try:
        conn.request("GET", path)
        response = conn.getresponse()
        return response.status, response.read(4096)
    finally: conn.close()

def healthy():
    for _ in range(30):
        try:
            if probe("/health")[0] == 200: return
        except (OSError, http.client.HTTPException): pass
        time.sleep(1)
    raise RuntimeError("gateway health check failed")

live = inspect()
target = json.loads(run("docker", "image", "inspect", image))[0]["Id"]
original = compose_file.read_text()
lines = original.splitlines(keepends=True)
start = lines.index("  fpl-mcp-bridge:\n")
end = next(i for i in range(start + 1, len(lines)) if lines[i].startswith("  ") and not lines[i].startswith("    "))
image_rows = [i for i in range(start + 1, end) if lines[i].startswith("    image: ")]
if len(image_rows) != 1 or lines[image_rows[0]].strip().split("image: ", 1)[1] != live["Config"]["Image"]:
    raise RuntimeError("live gateway and compose image differ")
stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
backup = root / "backups" / ("mcp-bridge-" + stamp)
backup.mkdir(mode=0o700)
(backup / "docker-compose.override.yaml").write_text(original)
os.chmod(backup / "docker-compose.override.yaml", 0o600)
lines[image_rows[0]] = f"    image: {image}\n"
try:
    write("".join(lines)); compose(); healthy()
    after = inspect()
    if after["Image"] != target:
        raise RuntimeError("deployed image identity changed")
    ignored = {"NODE_VERSION", "YARN_VERSION", "PATH", "NODE_ENV"}
    configured = lambda rows: sorted(row for row in rows if row.split("=", 1)[0] not in ignored)
    if configured(after["Config"]["Env"]) != configured(live["Config"]["Env"]):
        raise RuntimeError("container environment changed")
    key = lambda item: item["Destination"]
    if sorted(after["Mounts"], key=key) != sorted(live["Mounts"], key=key):
        raise RuntimeError("container mounts changed")
    if probe("/desktop/catalog")[0] != 401:
        raise RuntimeError("unauthenticated desktop catalog boundary failed")
except Exception as error:
    write(original); compose(); healthy()
    raise RuntimeError(f"gateway deployment failed ({error}); previous image restored") from None
print(json.dumps({"deployed": image, "configuration_preserved": True, "backup": str(backup)}))
