import json
import urllib.request

with urllib.request.urlopen("http://127.0.0.1:8443/health/ready", timeout=10) as response:
    if not json.load(response).get("ready"):
        raise SystemExit(1)
