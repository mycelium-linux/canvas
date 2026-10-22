#!/bin/sh
# Run on Agora after building Dock and the existing FPL Desktop Bridge owner CLI.
set -eu
mcp_source=${1:-"$HOME/.local/src/canvas-linux"}
test -x "$mcp_source/target/debug/canvas-linux"
mkdir -p "$HOME/.local/bin" "$HOME/.config/systemd/user"
install -m 755 "$mcp_source/target/debug/canvas-linux" "$HOME/.local/bin/canvas-linux.new"
mv -f "$HOME/.local/bin/canvas-linux.new" "$HOME/.local/bin/canvas-linux"
python3 - <<'PY'
import json,os,pathlib,tempfile,secrets
home=pathlib.Path.home()
config=pathlib.Path(os.environ.get('XDG_CONFIG_HOME',str(home/'.config')))/'FPL Desktop Bridge/config.json'
cfg=json.loads(config.read_text())
for project,port in [('dock',18760),('studio',18761)]:
    ident='dock-desktop-'+project
    secret_dir=home/'.local/state/dock/mcp'
    secret_dir.mkdir(mode=0o700,parents=True,exist_ok=True)
    secret_dir.chmod(0o700)
    secret=secret_dir/(project+'.token')
    if not secret.exists():
        fd=os.open(secret,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
        with os.fdopen(fd,'w') as out:out.write(secrets.token_hex(32))
    stat=secret.lstat()
    import stat as statmod
    if not statmod.S_ISREG(stat.st_mode) or stat.st_mode & 0o077 or stat.st_uid!=os.getuid():raise SystemExit('Unsafe local MCP credential permissions')
    desired={'id':ident,'title':'Dock '+project+' desktop','kind':'mcp-streamable-http','enabled':True,'localUrl':f'http://127.0.0.1:{port}/mcp','localAuthTokenFile':str(secret),'capabilities':['desktop.inspect','desktop.capture','desktop.input'],'metadata':{'app':'dock','adapter':'canvas-linux','capabilityId':'dock.desktop.'+project,'managedBy':'dock'}}
    existing=next((c for c in cfg['connectors'] if c['id']==ident),None)
    previous=dict(desired);previous.pop('localAuthTokenFile')
    if existing and existing!=desired and existing!=previous: raise SystemExit('Connector collision: '+ident+'; preserve owner configuration')
    if existing: existing.update(desired)
    else: cfg['connectors'].append(desired)
# Preserve arbitrary existing configuration and never persist a keyring secret.
with tempfile.NamedTemporaryFile(mode='w',dir=config.parent,delete=False) as out:
    os.chmod(out.name,0o600);json.dump(cfg,out,indent=2);out.flush();os.fsync(out.fileno());temporary=out.name
os.replace(temporary,config)
PY
for project in dock studio; do
    case "$project" in dock) port=18760;; studio) port=18761;; esac
    service="$HOME/.config/systemd/user/dock-mcp-$project.service"
    if test -f "$service" && ! grep -q '^# Managed by Dock MCP setup$' "$service"; then
        echo "Existing unit is not managed by Dock: $service" >&2; exit 1
    fi
    cat > "$service" <<UNIT
# Managed by Dock MCP setup
[Unit]
Description=Dock project MCP adapter ($project)
[Service]
Type=simple
ExecStart=%h/.local/bin/canvas-linux mcp-serve $project $port
Restart=on-failure
RestartSec=3
NoNewPrivileges=true
UMask=0077
[Install]
WantedBy=default.target
UNIT
done
owner_unit="$HOME/.config/systemd/user/fpl-desktop-bridge-dock.service"
if test -f "$owner_unit" && ! grep -q '^# Managed by Dock MCP setup$' "$owner_unit"; then
    echo "Existing owner unit is not managed by Dock" >&2; exit 1
fi
cat > "$owner_unit" <<'UNIT'
# Managed by Dock MCP setup
[Unit]
Description=FPL Desktop Bridge for Dock enrolled capabilities
After=network-online.target
[Service]
Type=simple
ExecStart=%h/.local/bin/fpl-desktop-bridge --config "%E/FPL Desktop Bridge/config.json"
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
UMask=0077
UNIT
systemctl --user daemon-reload
systemctl --user enable --now dock-mcp-dock.service dock-mcp-studio.service
# The owner daemon is started only after owner status verifies a real SSO session.
# Enrollment alone does not authenticate a browser or configure any model/session.
"$HOME/.local/bin/fpl-desktop-bridge" status
