"""Offline restore helper for the Docker migration bundle; runs inside the image."""
import argparse
import json
import pathlib
import re
import sqlite3
import tarfile
import tomllib
from contextlib import closing


def schedule(root, enabled):
    path = root / "config.toml"
    text = path.read_text(encoding="utf-8-sig")
    text, count = re.subn(r"(?m)^scheduler_enabled\s*=\s*(?:true|false)[ \t]*$",
                          "scheduler_enabled = " + str(enabled).lower(), text)
    if count != 1:
        raise ValueError("Expected exactly one scheduler_enabled setting")
    path.write_text(text, encoding="utf-8")
    path.chmod(0o600)


def restore(archive, root):
    if not archive.is_file():
        raise ValueError("Migration data archive is missing")
    if any(root.iterdir()):
        raise ValueError("Target data volume is not empty; existing data was not changed")
    with tarfile.open(archive, "r:*") as source:
        source.extractall(root, filter="data")
    config = tomllib.loads((root / "config.toml").read_text(encoding="utf-8-sig"))
    if not config.get("auth", {}).get("password_hash", "").startswith("$argon2id$"):
        raise ValueError("Backup is missing the configured administrator password hash")
    key = root / "state/credentials.key"
    if not key.is_file() or key.is_symlink() or len(key.read_bytes()) != 32:
        raise ValueError("Backup is missing its original 32-byte credential key")
    key.chmod(0o600)
    with closing(sqlite3.connect((root / "gamer.db").as_uri() + "?mode=ro", uri=True)) as db:
        if db.execute("PRAGMA integrity_check").fetchone()[0] != "ok":
            raise ValueError("Restored database integrity check failed")
        tasks = db.execute("SELECT count(*) FROM timer_tasks").fetchone()[0]
    schedule(root, False)
    print(json.dumps({"restored_tasks": tasks, "scheduler_enabled": False,
                      "next_step": "Verify browser logins, then run: bash restore.sh activate"}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("restore", "activate"))
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path("/data"))
    parser.add_argument("--archive", type=pathlib.Path, default=pathlib.Path("/migration/gamer-data.tar"))
    args = parser.parse_args()
    try:
        if args.mode == "restore":
            restore(args.archive, args.root)
        else:
            schedule(args.root, True)
            print("Automatic scheduling enabled; restart the service to apply")
    except Exception as error:
        # Avoid exposing credential bytes from parser or crypto exceptions.
        print(str(error) if isinstance(error, ValueError) else type(error).__name__)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
