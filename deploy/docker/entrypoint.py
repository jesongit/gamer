"""Initialize only runtime-owned paths, then exec Gamer as an unprivileged user."""
import os
import pathlib
import pwd
import shutil
import secrets
import re
import subprocess
import sys

os.umask(0o077)
root = pathlib.Path(os.environ.get("GAMER_DATA_DIR", "/data"))
if root.is_symlink():
    raise SystemExit("Data directory must not be a symlink")
root.mkdir(parents=True, exist_ok=True)
root.chmod(0o700)
config = pathlib.Path(os.environ.get("GB_CONFIG", "/data/config.toml"))
if config.is_symlink():
    raise SystemExit("Config must be a regular file")
if not config.exists():
    shutil.copyfile("/app/config.default.toml", config)
config.chmod(0o600)
key = pathlib.Path(os.environ["GAMER_SECRETS_KEY_FILE"])
key.parent.mkdir(parents=True, exist_ok=True)
if key.is_symlink():
    raise SystemExit("Credential key must be a regular file")
if not key.exists():
    for private in ("gamer-notify/private/channels.dat", "gamer-ocr/private/settings.dat",
                    "gamer-live/private/connection.dat", "gamer-live/private/sender.dat"):
        existing = root / "extension-data" / private
        if existing.is_file() and existing.read_bytes().startswith(b"GMRSEC01"):
            raise SystemExit("Credential key is missing; restore the original key with the data")
    with key.open("xb") as handle:
        handle.write(os.urandom(32))
if len(key.read_bytes()) != 32:
    raise SystemExit("Credential key must contain exactly 32 bytes; refusing to replace it")
key.chmod(0o600)
admin = root / "state/local-admin-token"
if admin.is_symlink():
    raise SystemExit("Local admin token must be a regular file")
if not admin.exists():
    with admin.open("x") as handle:
        handle.write(secrets.token_hex(32))
admin.chmod(0o600)
token = admin.read_text().strip()
if not re.fullmatch(r"[0-9a-fA-F]{64}", token):
    raise SystemExit("Local admin token is invalid")
os.environ["GAMER_ADMIN_TOKEN"] = token
home = root / "state/home"
if home.is_symlink():
    raise SystemExit("Runtime home must not be a symlink")
home.mkdir(parents=True, exist_ok=True)
home.chmod(0o700)
if os.getuid() == 0:
    account = pwd.getpwnam("gamer")
    # Named volumes are initially root-owned; avoid traversing them on every boot.
    if root.stat().st_uid != account.pw_uid:
        for current, dirs, files in os.walk(root, followlinks=False):
            for path in [pathlib.Path(current), *(pathlib.Path(current) / n for n in dirs + files)]:
                if not path.is_symlink():
                    os.chown(path, account.pw_uid, account.pw_gid)
    for path in (root, config, key.parent, key, admin, home):
        os.chown(path, account.pw_uid, account.pw_gid)
    os.initgroups(account.pw_name, account.pw_gid)
    os.setgid(account.pw_gid)
    os.setuid(account.pw_uid)
os.environ["HOME"] = str(home)
if sys.argv[1:] == ["/app/gamer-server"]:
    # Initialize ADB once before concurrent startup probes/scans. Keys and gh
    # user settings live in the data volume across container replacement.
    try:
        subprocess.run(["/usr/bin/adb", "start-server"], check=False,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
    except subprocess.TimeoutExpired:
        pass
os.execvp(sys.argv[1], sys.argv[1:])
