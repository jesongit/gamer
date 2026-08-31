#!/usr/bin/env python3
"""Independently verify a QA-007 snapshot manifest, bytes and SQLite copy."""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
import sys
import time
from pathlib import Path


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(4 * 1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def manifest_digest(manifest: dict) -> str:
    unsigned = dict(manifest)
    unsigned["manifest_sha256"] = ""
    raw = (json.dumps(unsigned, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    return hashlib.sha256(raw).hexdigest()


def sqlite_integrity(path: Path) -> str:
    connection = sqlite3.connect(f"file:{path.as_posix()}?mode=ro&immutable=1", uri=True)
    try:
        return connection.execute("PRAGMA integrity_check").fetchone()[0]
    finally:
        connection.close()


def rel(path: Path, base: Path) -> str:
    return path.relative_to(base).as_posix()


def actual_files(base: Path, expected: set[str], live: bool = False) -> dict[str, Path]:
    result: dict[str, Path] = {}
    if not base.exists():
        return result
    candidates: list[tuple[str, Path]] = []
    if live:
        data_root = base / "data"
        if data_root.exists():
            candidates.extend(
                (f"data/{rel(path, data_root)}", path)
                for path in data_root.rglob("*")
                if path.is_file()
            )
        config = base / "config" / "config.toml"
        if config.is_file():
            candidates.append(("config/config.toml", config))
    else:
        candidates.extend(
            (rel(path, base), path) for path in base.rglob("*") if path.is_file()
        )
    for name, path in candidates:
        if name == "manifest.json":
            continue
        lower = name.lower()
        if lower.endswith("-wal") or lower.endswith("-shm"):
            stem = lower.rsplit("-", 1)[0]
            if stem in {item.lower() for item in expected}:
                continue
        if lower in result:
            raise RuntimeError(f"case-insensitive path collision: {name}")
        result[lower] = path
    return result


def verify_map(manifest: dict, base: Path, label: str) -> dict:
    expected = {item["path"].lower(): item for item in manifest["files"]}
    actual = actual_files(
        base,
        set(item["path"] for item in manifest["files"]),
        live=label == "live",
    )
    missing = sorted(set(expected) - set(actual))
    extra = sorted(set(actual) - set(expected))
    if missing or extra:
        raise RuntimeError(f"{label} file set mismatch: missing={missing[:3]} extra={extra[:3]}")

    started = time.perf_counter()
    total = 0
    for key, item in expected.items():
        path = actual[key]
        size = path.stat().st_size
        if size != item["size"]:
            raise RuntimeError(f"{label} size mismatch: {item['path']}")
        digest = sha256_file(path)
        if digest != item["sha256"]:
            raise RuntimeError(f"{label} sha256 mismatch: {item['path']}")
        total += size
    elapsed = time.perf_counter() - started
    if total != manifest["total_bytes"]:
        raise RuntimeError(f"{label} total_bytes mismatch: {total} != {manifest['total_bytes']}")
    print(
        f"[verify:{label}] files={len(expected)} bytes={total:,} "
        f"mismatch=0 elapsed={elapsed:.1f}s"
    )
    return {"file_count": len(expected), "total_bytes": total, "seconds": round(elapsed, 3)}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    parser.add_argument("update_id")
    parser.add_argument("--live", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    backup = root / "backups" / args.update_id
    manifest_path = backup / "manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest["update_id"] != args.update_id:
        raise RuntimeError("manifest update_id mismatch")
    expected_digest = manifest_digest(manifest)
    if manifest["manifest_sha256"] != expected_digest:
        raise RuntimeError(
            "manifest self-hash mismatch: "
            f"{manifest['manifest_sha256']} != {expected_digest}"
        )
    if manifest["file_count"] != len(manifest["files"]):
        raise RuntimeError("manifest file_count mismatch")
    print(
        f"[verify] manifest update_id={args.update_id} "
        f"file_count={manifest['file_count']} total_bytes={manifest['total_bytes']:,} "
        f"manifest_sha256={manifest['manifest_sha256']}"
    )
    snapshot_report = verify_map(manifest, backup, "snapshot")
    for db in (backup / "data").rglob("*.db"):
        result = sqlite_integrity(db)
        print(f"[verify] snapshot integrity_check {db.name}: {result}")
        if result != "ok":
            raise RuntimeError("snapshot SQLite integrity_check failed")

    live_report = None
    if args.live:
        live_report = verify_map(manifest, root, "live")
        for db in (root / "data").rglob("*.db"):
            result = sqlite_integrity(db)
            print(f"[verify] live integrity_check {db.name}: {result}")
            if result != "ok":
                raise RuntimeError("live SQLite integrity_check failed")
    print(json.dumps({"snapshot": snapshot_report, "live": live_report}, sort_keys=True))
    print("[verify] PASS")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, sqlite3.Error, json.JSONDecodeError) as error:
        print(f"[verify] FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
