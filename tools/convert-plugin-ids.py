"""One-time, offline conversion of official plugin identities. Python >= 3.11.

Default is a dry run. --apply --server-stopped stages a complete copy, validates
it, and swaps directories. The untouched original directory becomes the backup.
No compatibility layer is installed in Gamer. Script bodies and task payloads
are deliberately opaque. Docker data is never discovered or modified.
"""
from __future__ import annotations
import argparse
from contextlib import closing
import datetime
import hashlib
import json
from pathlib import Path
import re
import shutil
import sqlite3
import tomllib
import zipfile

IDS = {"gamer.yaml": "gamer-yaml", "gamer.keymap": "gamer-keymap", "gamer.video": "gamer-video"}


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def convert_manifest(path, package=False):
    text = path.read_text(encoding="utf-8")
    parsed = tomllib.loads(text)
    if package:
        plugins = parsed.get("plugins", {})
        for old, new in IDS.items():
            if old in plugins and new in plugins:
                raise ValueError(f"Conflicting plugin dependencies: {path}")
            # Supported manifest serializer form: [plugins."id"]. Reject other
            # old-ID declarations instead of silently dropping a dependency.
            text = re.sub(r'(?m)^(\s*\[plugins\.)([\"\x27])' + re.escape(old) + r'\2(\]\s*(?:#.*)?)$',
                          lambda m: m[1] + m[2] + new + m[2] + m[3], text)
        updated = tomllib.loads(text)
        if any(old in updated.get("plugins", {}) for old in IDS):
            raise ValueError(f"Noncanonical plugin table; rewrite as [plugins.\"id\"] first: {path}")
    else:
        # Only identity declarations, never descriptions, parameters or code.
        for old, new in IDS.items():
            text = re.sub(r'(?m)^(\s*(?:id|builtin_id)\s*=\s*)([\"\x27])' + re.escape(old) + r'\2',
                          lambda m: m[1] + m[2] + new + m[2], text)
        tomllib.loads(text)
    path.write_text(text, encoding="utf-8")


def install_built_archive(root, plugin_id, registry_root):
    registry = read_json(registry_root / "registry.json")
    matches = [item for item in registry["plugins"] if item["id"] == plugin_id]
    if len(matches) != 1:
        raise ValueError(f"Expected one built artifact for {plugin_id}")
    entry = matches[0]
    archive = registry_root / "plugins" / Path(entry["download_url"]).name
    if hashlib.sha256(archive.read_bytes()).hexdigest() != entry["sha256"]:
        raise ValueError(f"Artifact checksum mismatch: {archive}")
    destination = root / "extensions" / plugin_id / entry["version"]
    destination.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive) as source:
        for info in source.infolist():
            target = (destination / info.filename).resolve()
            if not target.is_relative_to(destination.resolve()) or "\\" in info.filename or info.is_dir():
                raise ValueError(f"Unexpected archive path: {info.filename}")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(source.read(info))
    manifest = tomllib.loads((destination / "manifest.toml").read_text(encoding="utf-8"))
    if manifest["id"] != plugin_id or manifest["version"] != entry["version"]:
        raise ValueError("Artifact manifest identity mismatch")
    return entry["version"]


def convert_staged(root, registry_root=None):
    changes = {"directories": 0, "runner_references": 0, "media_references": 0, "installed_plugins": 0}
    package_roots = [p for p in (root / "packages").glob("*") if p.is_dir()]
    scoped_roots = [p / "plugins" for p in package_roots] + [root / "extensions"]
    for parent in scoped_roots:
        for old, new in IDS.items():
            source, destination = parent / old, parent / new
            if source.exists():
                if destination.exists():
                    raise ValueError(f"Refusing to overwrite existing plugin directory: {destination}")
                source.rename(destination)
                changes["directories"] += 1
    for package in package_roots:
        if (package / "package.toml").exists():
            convert_manifest(package / "package.toml", package=True)
        # Preset runner_id is a top-level scalar (PackagePreset contract). Do
        # not rewrite identically named keys in nested payloads or automations.
        for preset in (package / "plugins").glob("*/presets/*.yaml"):
            text = preset.read_text(encoding="utf-8")
            for old, new in IDS.items():
                text = re.sub(r'(?m)^(runner_id:\s*)([\"\x27]?)' + re.escape(old) + r'\2(\s*(?:#.*)?)$',
                              lambda m: m[1] + m[2] + new + m[2] + m[3], text)
            preset.write_text(text, encoding="utf-8")
    for manifest in (root / "extensions").glob("*/*/manifest.toml"):
        convert_manifest(manifest)
    state_path = root / "extensions/state.json"
    if state_path.exists():
        state = read_json(state_path)
        for old, new in IDS.items():
            if old not in state.get("plugins", {}):
                continue
            if new in state["plugins"]:
                raise ValueError(f"Conflicting extension lifecycle records: {new}")
            record = state["plugins"].pop(old)
            record["id"] = new
            if registry_root:
                record["active_version"] = install_built_archive(root, new, registry_root)
            state["plugins"][new] = record
            changes["installed_plugins"] += 1
        write_json(state_path, state)
    for metadata in (root / "media").glob("*/metadata.json"):
        value = read_json(metadata)
        changed = False
        for ref in value.get("refs", []):
            if ref.get("plugin_id") in IDS:
                ref["plugin_id"] = IDS[ref["plugin_id"]]
                changes["media_references"] += 1
                changed = True
        if changed:
            write_json(metadata, value)
    db_path = root / "gamer.db"
    if db_path.exists():
        with closing(sqlite3.connect(db_path)) as db:
            tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            for table in ["timer_tasks", "task_presets"]:
                if table not in tables:
                    continue
                for old, new in IDS.items():
                    changes["runner_references"] += db.execute(
                        f"UPDATE {table} SET runner_id=? WHERE runner_id=?", (new, old)).rowcount
            if db.execute("PRAGMA integrity_check").fetchone()[0] != "ok":
                raise ValueError("SQLite integrity check failed")
            db.commit()
            db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    return changes


def convert(data_root, *, apply=False, server_stopped=False, registry_root=None):
    root = Path(data_root).resolve(strict=True)
    if not root.is_dir() or root.parent == root:
        raise ValueError("Expected an explicit data directory")
    if apply and not server_stopped:
        raise ValueError("Stop Gamer, then supply --server-stopped; live conversion is forbidden")
    # Symlinks/junctions must never escape the explicitly selected data root.
    for path in root.rglob("*"):
        if path.is_symlink() or (hasattr(path, 'is_junction') and path.is_junction()):
            raise ValueError(f"Linked data path is unsupported: {path}")
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    staging = root.with_name(root.name + ".plugin-convert-" + stamp)
    backup = root.with_name(root.name + ".before-plugin-ids-" + stamp)
    size = sum(p.stat().st_size for p in root.rglob("*") if p.is_file())
    if shutil.disk_usage(root.parent).free < size + 128 * 1024 * 1024:
        raise ValueError("Insufficient space for a complete staging copy")
    shutil.copytree(root, staging)
    try:
        if (root / "gamer.db").exists():
            # Consistent SQLite snapshot, including committed WAL pages, also
            # makes dry runs safe while the old server is still running.
            snapshot = staging / "gamer.snapshot.db"
            with closing(sqlite3.connect(f"file:{(root / 'gamer.db').as_posix()}?mode=ro", uri=True)) as source, closing(sqlite3.connect(snapshot)) as target:
                source.backup(target)
            for suffix in ["", "-wal", "-shm"]:
                (staging / ("gamer.db" + suffix)).unlink(missing_ok=True)
            snapshot.rename(staging / "gamer.db")
        changes = convert_staged(staging, Path(registry_root).resolve() if registry_root else None)
        if not apply:
            return {"dry_run": True, "changes": changes}
        if not any(changes.values()):
            return {"applied": False, "reason": "already converted", "changes": changes}
        root.rename(backup)
        try:
            staging.rename(root)
        except BaseException:
            backup.rename(root)
            raise
        return {"applied": True, "backup": str(backup), "changes": changes}
    finally:
        if staging.exists():
            # A fully resolved sibling we created ourselves, never user data.
            assert staging.parent == root.parent and staging.name.startswith(root.name + ".plugin-convert-")
            shutil.rmtree(staging)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", required=True)
    parser.add_argument("--registry-dir", help="Built web/public directory; replace official installed runtime artifacts")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--server-stopped", action="store_true")
    args = parser.parse_args()
    print(json.dumps(convert(args.data_dir, apply=args.apply, server_stopped=args.server_stopped,
                             registry_root=args.registry_dir), ensure_ascii=False, indent=2))
