"""Offline data preparation and volume transfer. Never prints credentials.

Requires Python 3.11+ and cryptography only for prepare. Stop the source before
the final snapshot. Backup directories are never used as live data directories.
"""
import argparse
from contextlib import closing
import ctypes
import csv
import io
import hashlib
import getpass
import json
import os
import pathlib
import secrets
import shutil
import sqlite3
import subprocess
import sys
import tarfile
import tomllib

MAGIC = b"GMRSEC01"
PRIVATE_FILES = (
    "extension-data/gamer-notify/private/channels.dat",
    "extension-data/gamer-ocr/private/settings.dat",
    "extension-data/gamer-live/private/connection.dat",
    "extension-data/gamer-live/private/sender.dat",
)


def private_permissions(path, directory=False):
    """POSIX modes and Windows ACLs for newly created migration secrets only."""
    path.chmod(0o700 if directory else 0o600)
    if sys.platform == "win32":
        identity = subprocess.check_output(["whoami", "/user", "/fo", "csv", "/nh"], text=True)
        sid = next(csv.reader(io.StringIO(identity.strip())))[1]
        rights = "(OI)(CI)F" if directory else "F"
        subprocess.run(["icacls", str(path), "/inheritance:r", "/grant:r",
                        "*" + sid + ":" + rights, "*S-1-5-18:" + rights],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def dpapi_decrypt(data):
    if sys.platform != "win32":
        raise ValueError("Windows DPAPI data must first be exported by its original Windows account")
    from ctypes import wintypes
    class Blob(ctypes.Structure):
        _fields_ = [("size", wintypes.DWORD), ("data", ctypes.POINTER(ctypes.c_ubyte))]
    buffer = (ctypes.c_ubyte * len(data)).from_buffer_copy(data)
    source, output = Blob(len(data), buffer), Blob()
    decrypt = ctypes.windll.crypt32.CryptUnprotectData
    decrypt.argtypes = [ctypes.POINTER(Blob), ctypes.c_void_p, ctypes.c_void_p,
                        ctypes.c_void_p, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(Blob)]
    decrypt.restype = wintypes.BOOL
    if not decrypt(ctypes.byref(source), None, None, None, None, 1, ctypes.byref(output)):
        raise ValueError("Windows could not decrypt the source credential")
    try:
        return ctypes.string_at(output.data, output.size)
    finally:
        free = ctypes.windll.kernel32.LocalFree
        free.argtypes, free.restype = [ctypes.c_void_p], ctypes.c_void_p
        free(output.data)


def seal(key, data):
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    nonce = secrets.token_bytes(12)
    return MAGIC + nonce + AESGCM(key).encrypt(nonce, data, MAGIC)


def unseal(key, data):
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    if not data.startswith(MAGIC):
        raise ValueError("Unsupported credential envelope")
    return AESGCM(key).decrypt(data[8:20], data[20:], MAGIC)


def toml_document(values):
    def scalar(value):
        if isinstance(value, bool):
            return str(value).lower()
        if isinstance(value, (int, float)):
            return str(value)
        if isinstance(value, str):
            return json.dumps(value, ensure_ascii=False)
        if isinstance(value, list):
            return "[" + ", ".join(scalar(item) for item in value) + "]"
        raise ValueError("Unsupported config field type")
    lines = []
    def table(data, prefix=""):
        if prefix:
            lines.append("\n[" + prefix + "]")
        for name, value in data.items():
            if not isinstance(value, dict):
                lines.append(name + " = " + scalar(value))
        for name, value in data.items():
            if isinstance(value, dict):
                table(value, prefix + "." + name if prefix else name)
    table(values)
    return "\n".join(lines) + "\n"


def inventory(root):
    result = {}
    for current, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            path = pathlib.Path(current) / name
            if path.is_symlink():
                raise ValueError("Refusing a symlink in the data snapshot")
        for name in files:
            path = pathlib.Path(current) / name
            digest = hashlib.sha256()
            with path.open("rb") as handle:
                for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                    digest.update(chunk)
            result[path.relative_to(root).as_posix()] = {"size": path.stat().st_size, "sha256": digest.hexdigest()}
    return result


def prepare(args):
    source, destination = pathlib.Path(args.source).resolve(), pathlib.Path(args.destination).resolve()
    if destination.exists() or source == destination or source in destination.parents:
        raise ValueError("Destination must be a new directory outside the source data")
    if not (source / "gamer.db").is_file():
        raise ValueError("Source gamer.db is missing")
    config_path = pathlib.Path(args.config).resolve()
    config = tomllib.loads(config_path.read_text(encoding="utf-8-sig"))
    if not config.get("auth", {}).get("password_hash", "").startswith("$argon2id$"):
        raise ValueError("Production requires the existing Argon2id password hash")
    destination.mkdir(parents=True)
    private_permissions(destination, directory=True)
    backup, working = destination / "backup", destination / "data"
    # Snapshot every user file. The SQLite backup API includes committed WAL data.
    shutil.copytree(source, backup, symlinks=True,
                    ignore=lambda folder, names: [n for n in names if pathlib.Path(folder).resolve() == source and n in ("gamer.db", "gamer.db-wal", "gamer.db-shm")])
    with closing(sqlite3.connect((source / "gamer.db").as_uri() + "?mode=ro", uri=True)) as db:
        with closing(sqlite3.connect(backup / "gamer.db")) as target:
            db.backup(target)
            if target.execute("PRAGMA integrity_check").fetchone()[0] != "ok":
                raise ValueError("Database integrity check failed")
    shutil.copyfile(config_path, destination / "source-config.toml")
    manifest = inventory(backup)
    (destination / "backup-manifest.json").write_text(json.dumps(manifest, ensure_ascii=False), encoding="utf-8")
    shutil.copytree(backup, working)
    # Original browser profiles remain in the immutable backup. A Linux native
    # profile receives portable site storage and browser-exported session cookies.
    profiles = working / "browser-profiles"
    if profiles.exists():
        archive = working / "migration-originals" / "windows-browser-profiles"
        archive.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(profiles), archive)
        profiles.mkdir()
        with closing(sqlite3.connect(working / "gamer.db")) as db:
            targets = [json.loads(row[0]) for row in db.execute("SELECT config FROM browser_targets")]
        for target in targets:
            old = archive / target["profile_id"] / "Default"
            new = profiles / target["profile_id"] / "Default"
            new.mkdir(parents=True)
            for name in ("Local Storage", "IndexedDB", "Session Storage"):
                if (old / name).is_dir():
                    shutil.copytree(old / name, new / name)
    key = secrets.token_bytes(32)
    key_path = working / "state/credentials.key"
    key_path.parent.mkdir(exist_ok=True)
    if key_path.exists():
        original_key_path = working / "migration-originals/state/credentials.key"
        original_key_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(key_path, original_key_path)
    key_path.write_bytes(key)
    key_path.chmod(0o600)
    converted = []
    for relative in PRIVATE_FILES:
        path = working / relative
        if not path.exists():
            continue
        data = path.read_bytes()
        if data.startswith(MAGIC):
            original_key = source / "state/credentials.key"
            if not original_key.exists():
                original_key = (source / relative).with_name(".credentials.key")
            plain = unseal(original_key.read_bytes(), data)
        elif sys.platform == "win32":
            plain = dpapi_decrypt(data)
        else:
            # Explicit migration of legacy Unix JSON is permitted only here.
            plain = data
        parsed = json.loads(plain)
        if "gamer-ocr" in relative:
            parsed["engine_path"] = "/usr/bin/tesseract"
            parsed["tessdata_path"] = "/usr/share/tesseract-ocr/5/tessdata"
        path.write_bytes(seal(key, json.dumps(parsed, ensure_ascii=False).encode()))
        path.chmod(0o600)
        converted.append(relative)
    config.update(port=8443, local_only=False, scheduler_enabled=False, data_dir="/data",
                  browser_path="/usr/bin/chromium", adb_path="/usr/bin/adb",
                  ffmpeg_path="/usr/bin/ffmpeg", scrcpy_server="/app/assets/scrcpy-server.jar")
    config.setdefault("update", {})["strategy"] = "off"
    (working / "config.toml").write_text(toml_document(config), encoding="utf-8")
    summary = {"files": len(manifest), "bytes": sum(item["size"] for item in manifest.values()),
               "converted_credentials": converted, "scheduler_enabled": False}
    (destination / "preparation.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print(json.dumps(summary))


def load(args):
    source = pathlib.Path(args.prepared).resolve() / "data"
    if not (source / "config.toml").is_file():
        raise ValueError("Prepared runtime data directory is missing")
    subprocess.run(["docker", "volume", "create", args.volume], check=True, stdout=subprocess.DEVNULL)
    code = "import pathlib,sys,tarfile; p=pathlib.Path('/data'); assert not any(p.iterdir()), 'Volume is not empty'; tarfile.open(fileobj=sys.stdin.buffer,mode='r|').extractall(p,filter='data')"
    proc = subprocess.Popen(["docker", "run", "--rm", "--memory", "512m", "--memory-swap", "512m", "-i", "--user", "0", "--entrypoint", "python3",
                             "--mount", "type=volume,src=" + args.volume + ",dst=/data",
                             args.image, "-c", code], stdin=subprocess.PIPE)
    try:
        with tarfile.open(fileobj=proc.stdin, mode="w|") as archive:
            for path in source.iterdir():
                archive.add(path, arcname=path.name, recursive=True)
        proc.stdin.close()
    except BaseException:
        proc.kill()
        proc.wait()
        raise
    if proc.wait() != 0:
        raise ValueError("Volume import failed; partial volume was retained for inspection")
    print(json.dumps({"volume": args.volume, "imported": True}))


def backup(args):
    subprocess.run(["docker", "volume", "inspect", args.volume], check=True, stdout=subprocess.DEVNULL)
    helper = pathlib.Path(__file__).resolve().parents[1] / "deploy/docker/volume-transfer.py"
    if not helper.is_file():
        raise ValueError("Run the updated host migration tool with deploy/docker/volume-transfer.py present")
    command = ["docker", "run", "--rm", "--memory", "512m", "--memory-swap", "512m", "--user", "0", "--entrypoint", "python3",
               "--mount", "type=volume,src=" + args.volume + ",dst=/data,readonly",
               "--mount", "type=bind,src=" + str(helper) + ",dst=/volume-transfer.py,readonly",
               args.image, "/volume-transfer.py"]
    if args.report:
        subprocess.run(command + ["report", "--profile", args.profile], check=True)
        return
    if not args.output:
        raise ValueError("--output is required unless --report is used")
    destination = pathlib.Path(args.output).resolve()
    if destination.exists():
        raise ValueError("Backup destination already exists")
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Caller must stop the production container first. Do not create a hot file copy.
    active = subprocess.check_output(["docker", "ps", "--filter", "volume=" + args.volume, "--format", "{{.ID}}"], text=True).strip()
    if active:
        raise ValueError("Stop containers using this volume before taking a consistent backup")
    partial = destination.with_name(destination.name + ".partial")
    # A failed export is never published under the final backup filename.
    with partial.open("xb") as output:
        try:
            private_permissions(partial)
            compress = ["--gzip"] if destination.suffix == ".gz" else []
            subprocess.run(command + ["export", "--profile", args.profile] + compress, stdout=output, check=True)
        except BaseException:
            output.close()
            partial.unlink()
            raise
    try:
        os.link(partial, destination)  # No overwrite if another backup won a race.
    finally:
        partial.unlink()
    print(json.dumps({"backup": str(destination), "bytes": destination.stat().st_size,
                      "profile": args.profile, "compression": "gzip" if compress else "none"}))


def restore(args):
    source = pathlib.Path(args.input).resolve()
    if not source.is_file():
        raise ValueError("Backup archive is missing")
    subprocess.run(["docker", "volume", "create", args.volume], check=True, stdout=subprocess.DEVNULL)
    code = "import pathlib,sys,tarfile; p=pathlib.Path('/data'); assert not any(p.iterdir()), 'Volume is not empty'; tarfile.open(fileobj=sys.stdin.buffer,mode='r|*').extractall(p,filter='data')"
    with source.open("rb") as stream:
        subprocess.run(["docker", "run", "--rm", "--memory", "512m", "--memory-swap", "512m", "-i", "--user", "0", "--entrypoint", "python3",
                        "--mount", "type=volume,src=" + args.volume + ",dst=/data",
                        args.image, "-c", code], stdin=stream, check=True)
    print(json.dumps({"volume": args.volume, "restored": True}))


def initialize(args):
    from argon2.low_level import hash_secret, Type
    root = pathlib.Path(args.data).resolve()
    root.mkdir(parents=True, exist_ok=True)
    config_path = root / "config.toml"
    template = config_path if config_path.exists() else pathlib.Path("/app/config.default.toml")
    config = tomllib.loads(template.read_text(encoding="utf-8"))
    if config.get("auth", {}).get("password_hash"):
        raise ValueError("Administrator password is already configured")
    password = getpass.getpass("New administrator password: ")
    if not 8 <= len(password.encode()) <= 1024:
        raise ValueError("Password must contain between 8 and 1024 UTF-8 bytes")
    if password != getpass.getpass("Confirm password: "):
        raise ValueError("Passwords do not match")
    config.setdefault("auth", {})["password_hash"] = hash_secret(
        password.encode(), secrets.token_bytes(16), time_cost=2,
        memory_cost=19456, parallelism=1, hash_len=32, type=Type.ID).decode()
    config_path.write_text(toml_document(config), encoding="utf-8")
    config_path.chmod(0o600)
    print("Administrator configured. Automatic scheduling remains disabled.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    p = commands.add_parser("prepare")
    p.add_argument("--source", required=True)
    p.add_argument("--config", required=True)
    p.add_argument("--destination", required=True)
    p.set_defaults(run=prepare)
    p = commands.add_parser("load")
    p.add_argument("--prepared", required=True)
    p.add_argument("--volume", required=True)
    p.add_argument("--image", required=True)
    p.set_defaults(run=load)
    p = commands.add_parser("backup")
    p.add_argument("--volume", required=True)
    p.add_argument("--image", required=True)
    p.add_argument("--output")
    p.add_argument("--profile", choices=("full", "production"), default="full")
    p.add_argument("--report", action="store_true", help="Read-only size estimate; does not require stopping the service")
    p.set_defaults(run=backup)
    p = commands.add_parser("restore")
    p.add_argument("--volume", required=True)
    p.add_argument("--image", required=True)
    p.add_argument("--input", required=True)
    p.set_defaults(run=restore)
    p = commands.add_parser("init")
    p.add_argument("--data", default="/data")
    p.set_defaults(run=initialize)
    args = parser.parse_args()
    try:
        args.run(args)
    except Exception as error:
        # Error types/our own controlled validation messages never contain secret bytes.
        print("Docker data operation failed: " + (str(error) if isinstance(error, ValueError) else type(error).__name__), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
