#!/usr/bin/env python3
"""Create signed app-only manifests for the local QA-007 rig."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--dist-dir", type=Path, required=True)
    parser.add_argument("--manifests-dir", type=Path, required=True)
    parser.add_argument("--keys-dir", type=Path, required=True)
    parser.add_argument("--versions", nargs="+", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo = args.repo_root.resolve()
    dist = args.dist_dir.resolve()
    manifests = args.manifests_dir.resolve()
    keys = args.keys_dir.resolve()
    sign_tool = repo / "release" / "packaging" / "sign-manifest.mjs"
    validate_tool = repo / "release" / "contracts" / "validate-manifest.mjs"
    private_key = keys / "dev-ed25519-1.private.pem"
    manifests.mkdir(parents=True, exist_ok=True)

    for version in args.versions:
        name = f"gamer-app-{version}-windows-x64.zip"
        artifact = dist / name
        if not artifact.is_file():
            raise RuntimeError(f"missing app artifact: {artifact}")
        manifest = {
            "schema_version": 1,
            "product": "gamebot",
            "release": {
                "version": version,
                "channel": "stable",
                "published_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                "minimum_launcher_version": "0.1.0",
                "minimum_upgrade_version": "0.1.0",
                "data_schema": 1,
                "rollback_floor": 1,
                "release_notes_url": f"https://example.invalid/releases/v{version}",
            },
            "platforms": {
                "windows-x86_64": {
                    "app": {
                        "artifact": {
                            "name": name,
                            "url": f"https://example.invalid/download/v{version}/{name}",
                            "size": artifact.stat().st_size,
                            "sha256": sha256_file(artifact),
                        },
                        "entrypoint": "gamer-server.exe",
                    },
                    "components": [],
                    "resources": {
                        "scrcpy_server": {
                            "version": "3.3.3",
                            "path": "assets/scrcpy-server.jar",
                            "sha256": sha256_file(
                                repo / "server" / "assets" / "scrcpy-server.jar"
                            ),
                            "binding": "application",
                        }
                    },
                }
            },
        }
        path = manifests / f"{version}.json"
        path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        signed = subprocess.run(
            [
                "node",
                str(sign_tool),
                "sign",
                str(path),
                "--key",
                str(private_key),
                "--key-id",
                "dev-ed25519-1",
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        if signed.returncode != 0:
            raise RuntimeError(f"sign failed for {version}: {signed.stdout}{signed.stderr}")
        checked = subprocess.run(
            [
                "node",
                str(validate_tool),
                "check",
                str(path),
                "--sig",
                str(manifests / f"{version}.sig"),
                "--keys-dir",
                str(keys),
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        if checked.returncode != 0:
            raise RuntimeError(f"validate failed for {version}: {checked.stdout}{checked.stderr}")
        print(f"[manifests] PASS {version} artifact_bytes={artifact.stat().st_size}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError) as error:
        print(f"[manifests] FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
