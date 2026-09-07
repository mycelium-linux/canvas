"""Run on Batty through stdin; replace only the FPL Auth image and validate it."""
import datetime, http.client, json, os, sqlite3, subprocess, sys, tempfile, time
from pathlib import Path

root = Path("/home/ajmwagar/fpl-auth")
override = root / "google-release.override.json"
image = sys.argv[1]

def run(*args):
    return subprocess.check_output(args, cwd=root, text=True, stderr=subprocess.PIPE)

def inspect():
    return json.loads(run("docker", "inspect", "fpl-auth-fpl-auth-1"))[0]

def write(text):
    stat = override.stat()
    fd, path = tempfile.mkstemp(dir=root, prefix=".auth-image-")
    try:
        os.fchmod(fd, stat.st_mode & 0o777)
        os.fchown(fd, stat.st_uid, stat.st_gid)
        with os.fdopen(fd, "w") as stream:
            stream.write(text); stream.flush(); os.fsync(stream.fileno())
        os.replace(path, override)
    finally:
        if os.path.exists(path): os.unlink(path)

def compose():
    run("docker", "compose", "-f", str(root / "docker-compose.yml"), "-f",
        str(override), "up", "-d", "--no-deps", "--pull", "never", "fpl-auth")

def probe(method, path, body=None, headers=None):
    conn = http.client.HTTPConnection("192.168.1.40", 4000, timeout=5)
    try:
        conn.request(method, path, body, headers or {})
        response = conn.getresponse()
        return response.status, response.read(4096)
    finally: conn.close()

def healthy():
    for _ in range(30):
        try:
            if probe("GET", "/health")[0] == 200: return
        except (OSError, http.client.HTTPException): pass
        time.sleep(1)
    raise RuntimeError("health check failed")

live = inspect()
target = json.loads(run("docker", "image", "inspect", image))[0]["Id"]
original = override.read_text()
document = json.loads(original)
assert document["services"]["fpl-auth"]["image"] == live["Config"]["Image"]
stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
backup = root / "backups" / ("auth-image-" + stamp)
backup.mkdir(mode=0o700)
(backup / "original-override.json").write_text(original)
os.chmod(backup / "original-override.json", 0o600)
with sqlite3.connect(root / "data/fpl-auth.db") as source, sqlite3.connect(backup / "fpl-auth.db") as dest:
    source.backup(dest)
document["services"]["fpl-auth"]["image"] = image
try:
    write(json.dumps(document, indent=2) + "\n"); compose(); healthy()
    after = inspect()
    assert after["Image"] == target
    assert sorted(after["Config"]["Env"]) == sorted(live["Config"]["Env"])
    key = lambda item: item["Destination"]
    assert sorted(after["Mounts"], key=key) == sorted(live["Mounts"], key=key)
    body = b'{"callback_port":43210,"native_nonce":"nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn"}'
    assert probe("POST", "/desktop-bridges/browser/start", body, {"Content-Type":"application/json"})[0] == 401
except Exception:
    write(original); compose(); healthy()
    raise RuntimeError("deployment failed; previous image restored") from None
print(json.dumps({"deployed": image, "configuration_preserved": True, "backup": str(backup)}))
