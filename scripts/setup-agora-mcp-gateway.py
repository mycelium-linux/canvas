#!/usr/bin/env python3
"""Provision Agora One's restricted, loopback-only tunnel to the MCP gateway."""

import base64
import json
import subprocess
import sys
from pathlib import Path


AGORA = "agora-one"
GATEWAY = "batty"
EXPECTED_HOST_KEY = (
    "batty ssh-ed25519 "
    "AAAAC3NzaC1lZDI1NTE5AAAAILZgJ4tM4hsMY76s6uzGkyTf8JVBzEhILAcY+hSiLCrR"
)
KEY_COMMENT = "dock-agora-mcp-gateway"


def ssh(host: str, program: str, source: str, *arguments: str) -> str:
    result = subprocess.run(
        ["ssh", "-o", "BatchMode=yes", host, program, "-", *arguments],
        input=source,
        text=True,
        capture_output=True,
    )
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip() or "remote command failed"
        raise RuntimeError(f"{host}: {detail}")
    return result.stdout


CREATE_KEY = r'''
import os, pathlib, stat, subprocess
root = pathlib.Path.home() / ".local/state/dock/mcp"
root.mkdir(mode=0o700, parents=True, exist_ok=True)
root.chmod(0o700)
key = root / "batty_tunnel_ed25519"
if not key.exists():
    subprocess.run(
        ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C",
         "dock-agora-mcp-gateway", "-f", str(key)],
        check=True,
    )
for path, mode in ((key, 0o600), (key.with_suffix(key.suffix + ".pub"), 0o644)):
    current = path.lstat()
    if not stat.S_ISREG(current.st_mode) or current.st_uid != os.getuid():
        raise SystemExit(f"unsafe tunnel identity: {path}")
    path.chmod(mode)
fields = key.with_suffix(key.suffix + ".pub").read_text().strip().split()
if len(fields) < 2 or fields[0] != "ssh-ed25519":
    raise SystemExit("unexpected tunnel public-key format")
print(" ".join(fields[:2]) + " dock-agora-mcp-gateway")
'''


AUTHORIZE_KEY = r'''
import base64, os, pathlib, stat, sys, tempfile
public_key = base64.b64decode(sys.argv[1]).decode()
fields = public_key.strip().split()
if len(fields) != 3 or fields[0] != "ssh-ed25519" or fields[2] != "dock-agora-mcp-gateway":
    raise SystemExit("invalid Agora tunnel public key")
ssh_dir = pathlib.Path.home() / ".ssh"
ssh_dir.mkdir(mode=0o700, exist_ok=True)
ssh_dir.chmod(0o700)
authorized = ssh_dir / "authorized_keys"
rows = authorized.read_text().splitlines() if authorized.exists() else []
marker = " dock-agora-mcp-gateway"
rows = [row for row in rows if not row.endswith(marker)]
restriction = (
    'no-agent-forwarding,no-X11-forwarding,no-pty,no-user-rc,'
    'permitopen="127.0.0.1:8792"'
)
rows.append(f"{restriction} {fields[0]} {fields[1]} {fields[2]}")
fd, temporary = tempfile.mkstemp(dir=ssh_dir, prefix=".authorized_keys-")
try:
    os.fchmod(fd, 0o600)
    with os.fdopen(fd, "w") as stream:
        stream.write("\n".join(rows) + "\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, authorized)
finally:
    if os.path.exists(temporary):
        os.unlink(temporary)
'''


INSTALL_TUNNEL = r"""
import json, os, pathlib, stat, subprocess, sys, tempfile, time, urllib.error, urllib.request
expected_host_key = base64.b64decode(sys.argv[1]).decode().strip()
home = pathlib.Path.home()
state = home / ".local/state/dock/mcp"
key = state / "batty_tunnel_ed25519"
known_hosts = state / "batty_known_hosts"
unit_dir = home / ".config/systemd/user"
unit_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
unit = unit_dir / "dock-mcp-gateway-tunnel.service"
marker = "# Managed by Dock MCP gateway setup\n"
if unit.exists() and not unit.read_text().startswith(marker):
    raise SystemExit(f"existing unit is not managed by Dock: {unit}")
for path in (key,):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise SystemExit(f"unsafe tunnel identity permissions: {path}")

def atomic_write(path, content, mode):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix="." + path.name + "-")
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "w") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)

atomic_write(known_hosts, expected_host_key + "\n", 0o600)
content = marker + f'''[Unit]
Description=Dock MCP gateway tunnel to Batty
After=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/ssh -NT -o BatchMode=yes -o ExitOnForwardFailure=yes -o IdentitiesOnly=yes -o IdentityFile=%h/.local/state/dock/mcp/batty_tunnel_ed25519 -o StrictHostKeyChecking=yes -o UserKnownHostsFile=%h/.local/state/dock/mcp/batty_known_hosts -o ServerAliveInterval=20 -o ServerAliveCountMax=3 -L 127.0.0.1:18792:127.0.0.1:8792 batty
Restart=always
RestartSec=3
NoNewPrivileges=true
UMask=0077

[Install]
WantedBy=default.target
'''
atomic_write(unit, content, 0o600)

config = home / ".config/FPL Desktop Bridge/config.json"
data = json.loads(config.read_text())
desktop = data.get("desktop")
if not isinstance(desktop, dict):
    raise SystemExit("owner desktop configuration is missing")
desktop["gatewayUrl"] = "http://127.0.0.1:18792"
atomic_write(config, json.dumps(data, indent=2) + "\n", 0o600)

subprocess.run(["systemctl", "--user", "daemon-reload"], check=True)
subprocess.run(
    ["systemctl", "--user", "enable", "--now", "dock-mcp-gateway-tunnel.service"],
    check=True,
)
for attempt in range(20):
    try:
        with urllib.request.urlopen("http://127.0.0.1:18792/health", timeout=2) as response:
            if response.status == 200:
                break
    except (OSError, urllib.error.URLError):
        if attempt == 19:
            raise SystemExit("tunneled gateway health check failed")
        time.sleep(0.5)
else:
    raise SystemExit("tunneled gateway health check failed")
try:
    urllib.request.urlopen("http://127.0.0.1:18792/desktop/catalog", timeout=2)
    raise SystemExit("unauthenticated desktop catalog unexpectedly succeeded")
except urllib.error.HTTPError as error:
    if error.code != 401:
        raise SystemExit(f"unexpected unauthenticated catalog status: {error.code}")
print(json.dumps({
    "gateway_url": desktop["gatewayUrl"],
    "health": "ok",
    "unauthenticated_catalog_status": 401,
    "tunnel_service": "active",
}))
"""


def main() -> None:
    public_key = ssh(AGORA, "python3", CREATE_KEY).strip()
    encoded_key = base64.b64encode(public_key.encode()).decode()
    ssh(GATEWAY, "python3", AUTHORIZE_KEY, encoded_key)
    encoded_host_key = base64.b64encode(EXPECTED_HOST_KEY.encode()).decode()
    result = ssh(AGORA, "python3", "import base64\n" + INSTALL_TUNNEL, encoded_host_key)
    print(result.strip())


if __name__ == "__main__":
    try:
        main()
    except (OSError, subprocess.SubprocessError, RuntimeError) as error:
        print(f"MCP gateway setup failed: {error}", file=sys.stderr)
        raise SystemExit(1)
