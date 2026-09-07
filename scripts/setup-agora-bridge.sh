#!/bin/sh
# Run on Agora after building the existing FPL Desktop Bridge owner source.
set -eu
owner_source=${1:-"$HOME/.local/src/fpl-desktop-bridge"}
owner_binary="$owner_source/target/debug/fpl-desktop-bridge"
test -x "$owner_binary"
mkdir -p "$HOME/.local/lib/fpl-desktop-bridge" "$HOME/.local/bin"
install -m 755 "$owner_binary" "$HOME/.local/lib/fpl-desktop-bridge/bridge.new"
mv -f "$HOME/.local/lib/fpl-desktop-bridge/bridge.new" "$HOME/.local/lib/fpl-desktop-bridge/bridge"
cat > "$HOME/.local/bin/fpl-desktop-bridge" <<'LAUNCHER'
#!/bin/sh
# Use the desktop user's existing Secret Service, including from SSH.
export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
exec "$HOME/.local/lib/fpl-desktop-bridge/bridge" "$@"
LAUNCHER
chmod 755 "$HOME/.local/bin/fpl-desktop-bridge"
python3 - "$owner_source/config.example.json" <<'PY'
import json, os, pathlib, re, socket, subprocess, sys
root = pathlib.Path(os.environ.get('XDG_CONFIG_HOME', str(pathlib.Path.home()/'.config')))
path = root/'FPL Desktop Bridge/config.json'
if not path.exists():
    cfg = json.loads(pathlib.Path(sys.argv[1]).read_text())
    ip = subprocess.check_output(['tailscale','ip','-4'],text=True).strip()
    import ipaddress
    if ipaddress.ip_address(ip) not in ipaddress.ip_network('100.64.0.0/10'):
        raise SystemExit('Expected existing Tailscale IPv4 route')
    cfg['bridge'].update(id=re.sub('[^a-z0-9-]','-',socket.gethostname().lower()),displayName='Agora One',authToken='',listenAddress=ip+':8765',routeUrl='http://'+ip+':8765')
    cfg['controlPlane']['baseURL']='https://sso.fpl.dev'
    cfg['identity']['issuer']='https://sso.fpl.dev'
    for connector in cfg.get('connectors',[]): connector['enabled']=False
    path.parent.mkdir(parents=True,exist_ok=True)
    # Exclusive creation preserves any configuration another owner created meanwhile.
    with path.open('x') as output: json.dump(cfg,output,indent=2)
    path.chmod(0o600)
PY
"$HOME/.local/bin/fpl-desktop-bridge" status
