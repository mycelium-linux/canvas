"""Validate source artifacts before either platform consumes them.

Paths retain ~ for expansion on the executing host. Commands remain argv arrays;
the compiler never executes applications or invokes a shell.
"""

import hashlib
import json
import re
import tomllib
from pathlib import Path


class InvalidConfig(ValueError):
    """An actionable source configuration error."""


IDENTIFIER = re.compile(r"[a-z][a-z0-9-]*\Z")
COLOR = re.compile(r"#[0-9a-fA-F]{6}\Z")
DIMENSION = re.compile(r"(?:0|[1-9][0-9]*)(?:\.[0-9]+)?px\Z")
REQUIRED_COLORS = {"color-background", "color-surface", "color-text",
                   "color-text-muted", "color-brand", "color-border"}
REQUIRED_DIMENSIONS = {"space-2", "space-3", "radius-md", "font-size-sm"}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise InvalidConfig(message)


def string(value: object, context: str) -> str:
    require(isinstance(value, str) and bool(value.strip()), f"{context}: expected nonempty string")
    require(all(ord(c) >= 32 and ord(c) != 127 for c in value), f"{context}: control characters are forbidden")
    return value


def fields(value: object, required: set[str], optional: set[str], context: str) -> None:
    require(isinstance(value, dict), f"{context}: expected table")
    require(required <= value.keys(), f"{context}: missing fields {sorted(required - value.keys())}")
    require(value.keys() <= required | optional, f"{context}: unknown fields {sorted(value.keys() - required - optional)}")


def theme(path: Path) -> dict:
    raw = json.loads(path.read_text())
    require(isinstance(raw, dict), f"{path}: expected token object")
    require(REQUIRED_COLORS | REQUIRED_DIMENSIONS <= raw.keys(), f"{path}: missing required Coherence tokens")
    values = {}
    for key, token in raw.items():
        require(bool(IDENTIFIER.fullmatch(key)), f"{path}: invalid token name {key!r}")
        fields(token, {"$type", "$value"}, {"$description"}, f"{path}:{key}")
        value = string(token["$value"], f"{path}:{key}")
        kind = token["$type"]
        require(kind in ("color", "dimension"), f"{path}:{key}: unsupported token type")
        pattern = COLOR if kind == "color" else DIMENSION
        require(bool(pattern.fullmatch(value)), f"{path}:{key}: invalid {kind} value")
        if key in REQUIRED_COLORS:
            require(kind == "color", f"{path}:{key}: expected color")
        if key in REQUIRED_DIMENSIONS:
            require(kind == "dimension", f"{path}:{key}: expected dimension")
        values[key] = value
    return values


def profile(path: Path, colorways: set[str]) -> dict:
    raw = tomllib.loads(path.read_text())
    fields(raw, {"id", "name", "workspace", "session", "colorway", "apps"}, set(), str(path))
    for key in ("id", "name", "workspace", "session", "colorway"):
        string(raw[key], f"{path}:{key}")
    require(bool(IDENTIFIER.fullmatch(raw["id"])), f"{path}: invalid profile id")
    require(raw["colorway"] in colorways, f"{path}: unknown colorway {raw['colorway']!r}")
    require(isinstance(raw["apps"], list) and bool(raw["apps"]), f"{path}: apps must be a nonempty array")
    for index, app in enumerate(raw["apps"]):
        context = f"{path}:apps[{index}]"
        fields(app, {"platform", "argv", "cwd", "placement"}, set(), context)
        for key in ("platform", "cwd", "placement"):
            string(app[key], f"{context}:{key}")
        require(app["platform"] in ("macos", "linux"), f"{context}: platform must be macos or linux")
        require(app["placement"] in ("tiled", "floating", "fullscreen"), f"{context}: invalid placement")
        require(app["cwd"].startswith(("/", "~/")) or app["cwd"] == "~", f"{context}: cwd must be absolute or home-relative")
        require(isinstance(app["argv"], list) and bool(app["argv"]), f"{context}: argv must be a nonempty array")
        for arg in app["argv"]:
            string(arg, f"{context}:argv")
    return raw


def load(root: Path) -> dict:
    token_root = root / "design" / "coherence"
    manifest = json.loads((token_root / "manifest.json").read_text())
    fields(manifest, {"source", "colorways"}, set(), "Coherence manifest")
    string(manifest["source"], "Coherence source")
    require(isinstance(manifest["colorways"], dict) and len(manifest["colorways"]) >= 2,
            "Coherence manifest: at least two colorways required")
    colorways = {}
    for name, record in sorted(manifest["colorways"].items()):
        require(bool(IDENTIFIER.fullmatch(name)), "Coherence manifest: invalid colorway id")
        fields(record, {"file", "sha256"}, set(), f"colorway {name}")
        filename = string(record["file"], f"colorway {name}:file")
        require(Path(filename).name == filename and filename.endswith(".tokens.json"), "Token file must be a local .tokens.json filename")
        path = token_root / filename
        require(hashlib.sha256(path.read_bytes()).hexdigest() == record["sha256"], f"{path}: provenance checksum mismatch")
        colorways[name] = theme(path)
    profiles = [profile(path, set(colorways)) for path in sorted((root / "profiles").glob("*.toml"))]
    require(bool(profiles), "No TOML profiles found")
    ids = [item["id"] for item in profiles]
    require(len(set(ids)) == len(ids), "Duplicate profile id")
    return {"schema_version": 1, "colorways": colorways, "profiles": profiles}


def waybar_css(tokens: dict) -> str:
    colors = "\n".join(f"@define-color dock_{key.replace('-', '_')} {value};"
                       for key, value in sorted(tokens.items()) if key.startswith("color-"))
    return f"""/* Generated from Coherence; edit upstream tokens, then recompile. */
{colors}
window#waybar {{
  background-color: @dock_color_background;
  color: @dock_color_text;
  font-size: {tokens['font-size-sm']};
}}
#custom-dock-project, #custom-dock-colorway, #custom-dock-jarvis {{
  background-color: @dock_color_surface;
  padding: {tokens['space-2']} {tokens['space-3']};
  margin: {tokens['space-2']};
  border-radius: {tokens['radius-md']};
  border: 1px solid @dock_color_border;
}}
#custom-dock-project {{ color: @dock_color_brand; }}
#custom-dock-jarvis {{ color: @dock_color_text_muted; }}
"""


def compile_catalog(root: Path, output: Path) -> dict:
    catalog = load(root)
    # Render everything before any writes: malformed input cannot emit a partial catalog.
    artifacts = {"catalog.json": json.dumps(catalog, indent=2, sort_keys=True) + "\n"}
    artifacts.update({f"waybar-{name}.css": waybar_css(tokens)
                      for name, tokens in catalog["colorways"].items()})
    output.mkdir(parents=True, exist_ok=True)
    for name, content in artifacts.items():
        (output / name).write_text(content)
    return catalog
