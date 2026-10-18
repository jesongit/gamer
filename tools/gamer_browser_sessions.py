"""Transfer sessions from Gamer-owned browser profiles through normal browser CDP.

Requires websocket-client and cryptography (Debian: python3-websocket,
python3-cryptography). Bundles are encrypted; cookie values are never printed.
Run export before stopping the Windows source, import inside the new container
while automatic scheduling is disabled. This tool never starts a game task.
"""
import argparse
from contextlib import closing
import json
import os
import pathlib
import secrets
import sqlite3
import sys
import urllib.parse
import urllib.request

from gamer_docker import seal, unseal, private_permissions


def request(url, token=None, method="GET"):
    headers = {"X-Admin-Token": token} if token else {}
    req = urllib.request.Request(url, headers=headers, method=method,
                                 data=b"{}" if method == "POST" else None)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(req, timeout=40) as response:
        return json.load(response)


class Cdp:
    def __init__(self, profile):
        import websocket
        port = int((profile / "DevToolsActivePort").read_text().splitlines()[0])
        if not 0 < port < 65536:
            raise ValueError("Invalid local browser port")
        info = request(f"http://127.0.0.1:{port}/json/version")
        url = info["webSocketDebuggerUrl"]
        parsed = urllib.parse.urlsplit(url)
        if parsed.hostname not in ("127.0.0.1", "localhost") or parsed.port != port:
            raise ValueError("Browser debugging endpoint must be local")
        self.socket = websocket.create_connection(url, timeout=30, suppress_origin=True)
        self.counter = 0

    def call(self, method, params=None):
        self.counter += 1
        self.socket.send(json.dumps({"id": self.counter, "method": method, "params": params or {}}))
        while True:
            result = json.loads(self.socket.recv())
            if result.get("id") == self.counter:
                if "error" in result:
                    raise ValueError("Browser rejected session transfer: " + method)
                return result.get("result", {})

    def close(self):
        self.socket.close()


def targets(data):
    with closing(sqlite3.connect((data / "gamer.db").as_uri() + "?mode=ro", uri=True)) as db:
        result = [json.loads(row[0]) for row in db.execute(
            "SELECT config FROM browser_targets WHERE id IN (SELECT device_id FROM timer_tasks)")]
    for target in result:
        name = target["profile_id"]
        if not name or pathlib.PurePath(name).name != name or "/" in name or "\\" in name or name in (".", ".."):
            raise ValueError("Invalid Gamer browser profile")
    return result


def transfer(args):
    data = pathlib.Path(args.data).resolve()
    token = pathlib.Path(args.token_file).read_text().strip()
    api = args.api.rstrip("/")
    parsed = urllib.parse.urlsplit(api)
    if parsed.scheme != "http" or parsed.hostname not in ("localhost", "127.0.0.1", "::1"):
        raise ValueError("Session migration must use the local Gamer API")
    bundle, key_path = pathlib.Path(args.bundle), pathlib.Path(args.key)
    if args.command == "export":
        if bundle.exists() or key_path.exists():
            raise ValueError("Session bundle and key must be new files")
        sessions, key = {}, secrets.token_bytes(32)
    else:
        key = key_path.read_bytes()
        sessions = json.loads(unseal(key, bundle.read_bytes()))
    counts = {}
    for target in targets(data):
        target_id = target["id"]
        if args.command == "import" and target_id not in sessions:
            raise ValueError("A scheduled browser target is missing from the session bundle")
        if request(api + "/api/devices/" + urllib.parse.quote(target_id) + "/run", token).get("active"):
            raise ValueError("Stop active runs before transferring browser sessions")
        route = api + "/api/browser-targets/" + urllib.parse.quote(target_id)
        request(route + "/connect", token, "POST")
        cdp = None
        try:
            cdp = Cdp(data / "browser-profiles" / target["profile_id"])
            if args.command == "export":
                cookies = cdp.call("Storage.getCookies")["cookies"]
                sessions[target_id] = {"profile_id": target["profile_id"], "cookies": cookies}
            else:
                session = sessions[target_id]
                if session["profile_id"] != target["profile_id"]:
                    raise ValueError("Browser session profile does not match its target")
                # CookieParam excludes read-only Cookie fields. Retain security
                # attributes and partition keys, including session expiration.
                allowed = {"name", "value", "domain", "path", "expires", "httpOnly", "secure",
                           "sameSite", "priority", "sameParty", "sourceScheme", "sourcePort", "partitionKey"}
                cookies = []
                for original in session["cookies"]:
                    cookie = {k: v for k, v in original.items() if k in allowed}
                    if original.get("session") or cookie.get("expires", 0) <= 0:
                        cookie.pop("expires", None)
                    cookies.append(cookie)
                cdp.call("Storage.setCookies", {"cookies": cookies})
            counts[target_id] = len(cookies)
        finally:
            if cdp:
                cdp.close()
            request(route + "/disconnect", token, "POST")
    if args.command == "export":
        bundle.parent.mkdir(parents=True, exist_ok=True)
        key_path.parent.mkdir(parents=True, exist_ok=True)
        with key_path.open("xb") as handle:
            handle.write(key)
        private_permissions(key_path)
        with bundle.open("xb") as handle:
            handle.write(seal(key, json.dumps(sessions, ensure_ascii=False).encode()))
        private_permissions(bundle)
    print(json.dumps({"operation": args.command, "cookie_counts": counts}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("export", "import"))
    parser.add_argument("--data", required=True)
    parser.add_argument("--api", default="http://127.0.0.1:8443")
    parser.add_argument("--token-file", required=True)
    parser.add_argument("--bundle", required=True)
    parser.add_argument("--key", required=True)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        transfer(args)
    except Exception as error:
        print("Browser session transfer failed: " + (str(error) if isinstance(error, ValueError) else type(error).__name__), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
