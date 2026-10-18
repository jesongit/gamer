"""Read-only volume inventory/export, executed in the release image by the host tool.

The production profile omits only known local backup/diagnostic directories and
disposable Chromium caches. Unknown paths and all application history are kept.
"""
import argparse
import json
import os
import pathlib
import stat
import sys
import tarfile


LOCAL_ONLY = {
    "_backups": "local_backups",
    "migration-originals": "previous_platform_originals",
    "hsr-supervision": "local_diagnostics",
    "live-workflow-20261015": "local_diagnostics",
    "live-workflow-20261016": "local_diagnostics",
    "qa-autonomy": "local_diagnostics",
    "qa-cursor-lock": "local_diagnostics",
}
PROFILE_CACHES = {"ShaderCache", "GrShaderCache", "GraphiteDawnCache", "GPUCache"}
DEFAULT_CACHES = {"Cache", "Code Cache", "GPUCache", "DawnCache", "DawnGraphiteCache", "DawnWebGPUCache"}
PROFILE_TRANSIENT = {"SingletonLock", "SingletonSocket", "SingletonCookie", "DevToolsActivePort"}


def excluded_reason(relative, profile):
    if profile == "full":
        return None
    parts = pathlib.PurePosixPath(relative).parts
    if not parts:
        return None
    if parts[0] in LOCAL_ONLY:
        return LOCAL_ONLY[parts[0]]
    if len(parts) >= 3 and parts[0] == "browser-profiles":
        if parts[2] in PROFILE_CACHES | PROFILE_TRANSIENT:
            return "browser_cache_or_lock"
        if len(parts) >= 4 and parts[2] == "Default" and parts[3] in DEFAULT_CACHES:
            return "browser_cache_or_lock"
    return None


def entries(root):
    for current, directories, files in os.walk(root, followlinks=False):
        directories.sort()
        for name in sorted(directories + files):
            path = pathlib.Path(current) / name
            yield path, path.relative_to(root).as_posix(), path.lstat()


def report(root, profile):
    totals = {"profile": profile, "included_bytes": 0, "excluded_bytes": 0,
              "included_files": 0, "excluded_files": 0, "excluded_by_reason": {}}
    for path, relative, metadata in entries(root):
        if not stat.S_ISREG(metadata.st_mode):
            continue
        reason = excluded_reason(relative, profile)
        group = "excluded" if reason else "included"
        totals[group + "_bytes"] += metadata.st_size
        totals[group + "_files"] += 1
        if reason:
            totals["excluded_by_reason"][reason] = totals["excluded_by_reason"].get(reason, 0) + metadata.st_size
    print(json.dumps(totals))


def export(root, profile, stream, compressed):
    # One selection rule for inventory and export; no deletion or config changes.
    with tarfile.open(fileobj=stream, mode="w|gz" if compressed else "w|",
                      **({"compresslevel": 1} if compressed else {})) as archive:
        for path, relative, metadata in entries(root):
            if excluded_reason(relative, profile):
                continue
            if not (stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode)):
                raise ValueError("Unsupported link or special file in selected volume data")
            archive.add(path, arcname=relative, recursive=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("report", "export"))
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path("/data"))
    parser.add_argument("--profile", choices=("full", "production"), default="full")
    parser.add_argument("--gzip", action="store_true")
    args = parser.parse_args()
    if args.mode == "report":
        report(args.root, args.profile)
    else:
        export(args.root, args.profile, sys.stdout.buffer, args.gzip)


if __name__ == "__main__":
    main()
