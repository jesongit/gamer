#!/usr/bin/env python3
"""Prepare the QA-007 database and small-file fixture.

``real`` writes actual blob payloads into the server-created schema-v1
database.  It is the only mode allowed to enter the real launcher upgrade
path.  ``sparse`` creates a logical 1 GiB sparse file for preflight-only
evidence; it must never be described as a copied 1 GiB database.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
import random
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path


EXPECTED_TABLES = {"devices", "logs", "scheduled_runs", "tasks"}
DEFAULT_DB_BYTES = 1 << 30
DEFAULT_SMALL_FILES = 4096
DEFAULT_PACKAGE = "com.example.qastress"
BLOB_BYTES = 512 * 1024
SEED = 20260831


def sparse_flag(path: Path) -> str:
    """Return ``sparse``, ``not-sparse`` or ``unknown`` on Windows."""
    # FILE_ATTRIBUTE_SPARSE_FILE is 0x200.  Querying the attribute directly
    # avoids locale/redirected-console differences in fsutil's text output.
    if os.name == "nt":
        attributes = ctypes.windll.kernel32.GetFileAttributesW(str(path))
        if attributes != 0xFFFFFFFF:
            return "sparse" if attributes & 0x200 else "not-sparse"
    fsutil = shutil.which("fsutil.exe") or shutil.which("fsutil")
    if not fsutil:
        return "unknown"
    result = subprocess.run(
        [fsutil, "sparse", "queryflag", str(path)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    output = f"{result.stdout}\n{result.stderr}".lower()
    if "not sparse" in output:
        return "not-sparse"
    if "sparse" in output and result.returncode == 0:
        return "sparse"
    return "unknown"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(4 * 1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def exact_v1_tables(connection: sqlite3.Connection) -> None:
    tables = {
        row[0]
        for row in connection.execute(
            "SELECT name FROM sqlite_master "
            "WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"
        )
    }
    if tables != EXPECTED_TABLES:
        raise RuntimeError(
            f"server schema table set changed: expected={sorted(EXPECTED_TABLES)} "
            f"actual={sorted(tables)}"
        )


def remove_old_files(base: Path) -> None:
    for kind in ("tmpl", "func", "yaml"):
        directory = base / kind
        if not directory.exists():
            continue
        for path in directory.glob("qa-stage5b-*"):
            if path.is_file() or path.is_symlink():
                path.unlink()


def create_small_files(base: Path, count: int) -> int:
    for kind in ("tmpl", "func", "yaml"):
        (base / kind).mkdir(parents=True, exist_ok=True)
    remove_old_files(base)
    for index in range(count):
        kind = ("tmpl", "func", "yaml")[index % 3]
        path = base / kind / f"qa-stage5b-{index:05d}.bin"
        payload = (
            f"stage5b index={index:05d} kind={kind} "
            f"sha={hashlib.sha256(str(index).encode()).hexdigest()}\n"
        ).encode("ascii")
        path.write_bytes(payload)
    return sum(1 for path in base.rglob("qa-stage5b-*") if path.is_file())


def prepare_real(db: Path, target_bytes: int) -> tuple[int, str, float]:
    connection = sqlite3.connect(db)
    try:
        if connection.execute("PRAGMA user_version").fetchone()[0] != 1:
            raise RuntimeError("expected server-created SQLite schema user_version=1")
        exact_v1_tables(connection)
        connection.execute("PRAGMA journal_mode=DELETE")
        connection.execute("DELETE FROM logs WHERE device_id = 'qa-stage5b'")
        connection.commit()
        # Make repeated runs measure the new payload, not SQLite freelist
        # bytes left by a previous QA fill.
        connection.execute("VACUUM")

        started = time.perf_counter()
        generator = random.Random(SEED)
        connection.execute("BEGIN")
        row_count = 0
        while db.stat().st_size < target_bytes or row_count < 1:
            payload = generator.randbytes(BLOB_BYTES)
            connection.execute(
                "INSERT INTO logs(time, device_id, script_id, level, msg) "
                "VALUES (?, 'qa-stage5b', 'qa-stage5b/fill', 'info', ?)",
                (f"2026-08-31T12:00:{row_count % 60:02d}Z", payload),
            )
            row_count += 1
        connection.commit()
        elapsed = time.perf_counter() - started
        integrity = connection.execute("PRAGMA integrity_check").fetchone()[0]
        if integrity != "ok":
            raise RuntimeError(f"PRAGMA integrity_check returned {integrity!r}")
    finally:
        connection.close()

    flag = sparse_flag(db)
    if flag != "not-sparse":
        raise RuntimeError(
            "real mode cannot prove a materialized DB: "
            f"fsutil sparse flag={flag!r}"
        )
    return row_count, flag, elapsed


def prepare_sparse(db: Path, target_bytes: int) -> tuple[int, str, float]:
    started = time.perf_counter()
    if db.exists():
        db.unlink()
    db.parent.mkdir(parents=True, exist_ok=True)
    db.touch()
    fsutil = shutil.which("fsutil.exe") or shutil.which("fsutil")
    if not fsutil:
        raise RuntimeError("sparse mode requires Windows fsutil")
    result = subprocess.run(
        [fsutil, "sparse", "setflag", str(db)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"fsutil sparse setflag failed: {result.stderr}")
    with db.open("r+b") as target:
        target.truncate(target_bytes)
    flag = sparse_flag(db)
    if flag != "sparse":
        raise RuntimeError(f"sparse mode could not prove sparse flag: {flag!r}")
    return 0, flag, time.perf_counter() - started


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("rig", type=Path)
    parser.add_argument("--mode", choices=("real", "sparse"), default="real")
    parser.add_argument("--db-bytes", type=int, default=DEFAULT_DB_BYTES)
    parser.add_argument("--small-files", type=int, default=DEFAULT_SMALL_FILES)
    parser.add_argument("--package", default=DEFAULT_PACKAGE)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.db_bytes < DEFAULT_DB_BYTES:
        raise SystemExit("--db-bytes must be at least 1 GiB for QA-007")
    if args.small_files < 2048:
        raise SystemExit("--small-files must be at least 2048 for QA-007")

    rig = args.rig.resolve()
    db = rig / "data" / "gamer.db"
    if not db.is_file() and args.mode == "real":
        raise SystemExit(f"server-created database is missing: {db}")
    db.parent.mkdir(parents=True, exist_ok=True)

    if args.mode == "real":
        rows, flag, elapsed = prepare_real(db, args.db_bytes)
    else:
        rows, flag, elapsed = prepare_sparse(db, args.db_bytes)

    package_root = rig / "data" / args.package
    file_count = create_small_files(package_root, args.small_files)
    db_size = db.stat().st_size
    summary = {
        "mode": args.mode,
        "db_path": "data/gamer.db",
        "db_logical_bytes": db_size,
        "db_target_bytes": args.db_bytes,
        "db_rows": rows,
        "db_sparse_flag": flag,
        "small_file_count": file_count,
        "small_file_root": f"data/{args.package}",
        "fill_seconds": round(elapsed, 3),
        "sqlite_integrity_check": "ok" if args.mode == "real" else "not-run-sparse-fixture",
        "real_snapshot_copy_allowed": args.mode == "real",
    }
    marker = rig / "data" / ".qa-stage5b-profile.json"
    marker.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2))
    print(
        f"[stage5b] PASS mode={args.mode} db_bytes={db_size:,} "
        f"small_files={file_count} sparse_flag={flag}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, sqlite3.Error, RuntimeError) as error:
        print(f"[stage5b] FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
