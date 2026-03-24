#!/usr/bin/env python3

"""Prepare workspace Cargo manifests for crates.io publishing.

Usage:
    python3 scripts/update-versions.py

What this script does:
    - Reads the workspace version from the root Cargo.toml.
    - Scans each crate manifest in the workspace.
    - Finds internal Rex crate dependencies that currently rely on
      `workspace = true` or on a local `path = "../some-crate"` without a
      version.
    - Rewrites those dependency lines to include both a local path and an
      explicit version, and updates existing internal dependency versions when
      they do not match the current workspace version, for example:
          rexlang-core = { path = "../rexlang-core", version = "x.x.x" }

Why this exists:
    Cargo allows path-only workspace dependencies during normal local
    development, but published crates must declare a real version requirement
    for every published dependency. Without that, `cargo publish` fails while
    verifying the manifest.

Important behavior:
    - The script only targets workspace crates under the repository root.
    - External dependencies are left alone.
    - The script is idempotent: running it repeatedly should either make the
      same rewrite once or print "no changes needed".
"""

from __future__ import annotations

import re
from pathlib import Path


#
# The script lives in `scripts/`, but the Cargo workspace root is its parent.
# We resolve everything relative to the repository root so the command works
# whether you run it from the repo root or from somewhere else.
#
ROOT_DIR = Path(__file__).resolve().parent.parent
ROOT_MANIFEST = ROOT_DIR / "Cargo.toml"


def extract_package_name(manifest_text: str) -> str:
    """Return the crate name from a package manifest."""
    match = re.search(r'^\[package\].*?^\s*name\s*=\s*"([^"]+)"', manifest_text, re.MULTILINE | re.DOTALL)
    if not match:
        raise RuntimeError("could not determine package name")
    return match.group(1)


def extract_package_version(manifest_text: str, workspace_version: str) -> str:
    """Return the version a crate will publish with.

    Some crates declare `version = "..."` directly.
    Others inherit it via `version.workspace = true`, in which case we use the
    workspace package version from the root manifest.
    """
    explicit = re.search(
        r'^\[package\].*?^\s*version\s*=\s*"([^"]+)"',
        manifest_text,
        re.MULTILINE | re.DOTALL,
    )
    if explicit:
        return explicit.group(1)

    inherited = re.search(
        r'^\[package\].*?^\s*version\.workspace\s*=\s*true',
        manifest_text,
        re.MULTILINE | re.DOTALL,
    )
    if inherited:
        return workspace_version

    raise RuntimeError("could not determine package version")


def extract_workspace_version(root_text: str) -> str:
    """Read the shared workspace version from the root Cargo.toml."""
    match = re.search(
        r'^\[workspace\.package\].*?^\s*version\s*=\s*"([^"]+)"',
        root_text,
        re.MULTILINE | re.DOTALL,
    )
    if not match:
        raise RuntimeError("could not determine [workspace.package] version")
    return match.group(1)


def main() -> int:
    """Rewrite internal crate dependencies to include publishable versions."""
    if not ROOT_MANIFEST.exists():
        raise RuntimeError(f"could not find root Cargo.toml at {ROOT_MANIFEST}")

    root_text = ROOT_MANIFEST.read_text()
    workspace_version = extract_workspace_version(root_text)

    # Build a map of workspace crate name -> (version, directory name).
    # We use this in the second pass to decide which dependency lines are
    # internal workspace edges and what their `path` and `version` should be.
    crate_info: dict[str, tuple[str, str]] = {}
    for manifest in sorted(ROOT_DIR.glob("*/Cargo.toml")):
        text = manifest.read_text()
        name = extract_package_name(text)
        version = extract_package_version(text, workspace_version)
        crate_info[name] = (version, manifest.parent.name)

    updated_paths: list[Path] = []
    for manifest in sorted(ROOT_DIR.glob("*/Cargo.toml")):
        # We intentionally operate line by line because the dependency entries
        # we care about are single-line inline tables. This keeps the rewrite
        # small, predictable, and easy to inspect in git diff.
        lines = manifest.read_text().splitlines()
        changed = False
        rewritten: list[str] = []

        for line in lines:
            updated_line = line
            stripped = line.lstrip()

            for crate_name, (crate_version, crate_dir) in crate_info.items():
                prefix = f"{crate_name} = {{"
                if not stripped.startswith(prefix):
                    continue

                dep_path = f"../{crate_dir}"
                # Case 1: dependency comes from `[workspace.dependencies]` and
                # appears in the crate as `{ workspace = true }`. That is fine
                # for local development but not sufficient for publishing when
                # the workspace dependency ultimately resolves to a local path.
                if "workspace = true" in updated_line:
                    updated_line = updated_line.replace(
                        "workspace = true",
                        f'path = "{dep_path}", version = "{crate_version}"',
                        1,
                    )
                    changed = True
                # Case 2: dependency already has a local path. Ensure it also
                # carries the current workspace version. If the version is
                # missing, add it; if it exists but is stale, replace it.
                elif f'path = "{dep_path}"' in updated_line:
                    if "version =" in updated_line:
                        refreshed_line = re.sub(
                            r'version\s*=\s*"[^"]+"',
                            f'version = "{crate_version}"',
                            updated_line,
                            count=1,
                        )
                        if refreshed_line != updated_line:
                            updated_line = refreshed_line
                            changed = True
                    else:
                        updated_line = updated_line.replace(
                            f'path = "{dep_path}"',
                            f'path = "{dep_path}", version = "{crate_version}"',
                            1,
                        )
                        changed = True
                break

            rewritten.append(updated_line)

        if changed:
            manifest.write_text("\n".join(rewritten) + "\n")
            updated_paths.append(manifest)

    # Keep the output simple so it is easy to use in release notes or shell
    # scripts that want to know whether anything changed.
    if updated_paths:
        for path in updated_paths:
            print(f"updated {path.relative_to(ROOT_DIR)}")
    else:
        print("no changes needed")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
