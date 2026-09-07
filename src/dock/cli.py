"""Small deterministic CLI shared by platform build/run scripts."""

import argparse
import json
import sys
import tomllib
from pathlib import Path

from .catalog import InvalidConfig, compile_catalog, load


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate Dock profiles and compile Coherence artifacts")
    parser.add_argument("--root", type=Path, default=Path.cwd(), help="Dock source directory (default: current directory)")
    subcommands = parser.add_subparsers(dest="command", required=True)
    subcommands.add_parser("validate")
    subcommands.add_parser("profiles")
    compile_parser = subcommands.add_parser("compile")
    compile_parser.add_argument("--output", type=Path, default=Path("build"))
    args = parser.parse_args()
    try:
        if args.command == "compile":
            catalog = compile_catalog(args.root, args.output)
        else:
            catalog = load(args.root)
        if args.command == "profiles":
            print(json.dumps(catalog["profiles"], indent=2))
        else:
            print(f"Validated {len(catalog['profiles'])} profiles and {len(catalog['colorways'])} colorways"
                  + (f"; wrote artifacts to {args.output}" if args.command == "compile" else ""))
        return 0
    except (InvalidConfig, OSError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"dock: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
