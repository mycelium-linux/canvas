"""The shared loopback credential for Canvas's MCP servers.

Canvas serves two MCP surfaces on this machine — presentation from `canvas-mcp`
and workspace orchestration from `canvas-mac` — and both are the same
application on the same account behind the same loopback boundary. They read one
credential file rather than minting one each, so this lives in one place: when
the checks were copied per installer, only the installer someone happened to run
would have been the one that was correct.

The Rust side of the same contract is `canvas::loopback`; the path below and
`canvas::loopback::token_path` must name the same file.
"""

from __future__ import annotations

import os
import pathlib
import secrets
import stat

# Still `Dock`: this names a file already installed on this machine and pointed
# at by the Desktop Bridge connector config, so it moves in a deliberate
# migration rather than a rename. See the naming note in AGENTS.md.
TOKEN_RELATIVE = "Library/Application Support/Dock/mcp/token"


def token_path(home: pathlib.Path) -> pathlib.Path:
    return home / TOKEN_RELATIVE


def private_regular(path: pathlib.Path) -> bool:
    """True only for a real file this account owns that nobody else can read.

    `lstat` rather than `stat`: following a symlink here would check the
    permissions of the link and then read whatever it points at.
    """
    info = path.lstat()
    return stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and not info.st_mode & 0o077


def ensure_token(path: pathlib.Path) -> None:
    """Create the credential if absent; refuse an existing unsafe one.

    `O_EXCL` makes creation the atomic step, so two installers racing cannot both
    believe they minted the token and leave one server holding a stale copy.
    """
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.parent.chmod(0o700)
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        if not private_regular(path) or len(path.read_text()) != 64:
            raise SystemExit("Existing Canvas MCP token is unsafe or invalid")
        return
    with os.fdopen(descriptor, "w") as output:
        # 32 bytes hex-encoded is the 64 characters both servers validate.
        output.write(secrets.token_hex(32))
        output.flush()
        os.fsync(output.fileno())
