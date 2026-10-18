# Gamer

Gamer provides browser cloud gaming, YAML automation and scheduled tasks. The
image includes the server, static web UI, Chromium, FFmpeg, ADB and local English /
Simplified Chinese OCR. A separate browser or VNC container is not required.

## Release

```sh
docker pull --platform linux/amd64 posase/gamer:v1.0.1
```

Docker release **v1.0.1** packages application **0.4.0** with portable credentials,
production diagnostic retention and authenticated per-run diagnostic ZIPs.
The published architecture is **linux/amd64**, for Intel/AMD x86_64 hosts.
The previous `v1.0` tag remains available. No ARM64 release is claimed.

Source and deployment bundle:
https://github.com/jesongit/gamer/releases/tag/docker-v1.0.1

## Fresh installation with docker run

Download and extract the deployment bundle from the release above. It includes
`deploy/docker/chromium-seccomp.json` and requires no Compose.

```sh
docker volume create gamer-production-data
docker run --rm -it --memory 512m --memory-swap 512m --entrypoint python3 \
  --mount type=volume,src=gamer-production-data,dst=/data \
  posase/gamer:v1.0.1 /app/tools/gamer_docker.py init

docker run -d --name gamer --init --restart unless-stopped \
  --stop-timeout 90 --shm-size 1g \
  --security-opt seccomp=./deploy/docker/chromium-seccomp.json \
  -p 127.0.0.1:18443:8443 -e TZ=Asia/Shanghai \
  --mount type=volume,src=gamer-production-data,dst=/data \
  --log-driver json-file --log-opt max-size=10m --log-opt max-file=5 \
  posase/gamer:v1.0.1
```

Open `http://localhost:18443` using your configured password. There is no default
password. New installations start with automatic scheduling disabled. Chromium
runs sandboxed as UID 1000; application self-update is off. Browser idle reclamation
is 30 minutes. CDP remains internal and must not be published.

## Existing data migration

**Skip init when migrating existing data.** Restore the complete data context to
an empty named volume: database, tasks, packages, plugin installations, browser
profiles and the original credential key must remain together. The deployment
bundle's `restore.sh` validates the data and starts Gamer with scheduling disabled.
Verify account login and the task list, stop the old production instance, then run
`bash restore.sh activate`. Only one machine should schedule the same accounts.

Detailed steps: https://github.com/jesongit/gamer/blob/docker-v1.0.1/deploy/docker/MIGRATION.md

The optional compact transfer profile omits only explicitly listed local backup /
diagnostic copies and disposable browser caches. It preserves application history,
scripts, media, plugin data and credentials; it does not delete the source files.
The data archive contains sensitive account state and must be transferred privately.
A new machine or IP may require the provider's normal login verification.

## Production diagnostics and updates

New runs use sampled screenshots, bounded trace storage and independent success /
failure retention. Existing pre-policy evidence remains preserved. Completed runs
provide an authenticated diagnostic ZIP from Tasks > View logs. Images may contain
account information; review a package before sharing it.

Update by replacing the image and keeping `/data`. Back up before upgrading; retain
the previous image with its matching pre-upgrade volume for rollback. Do not attach
an upgraded data volume directly to an older image. Data and credential keys must
be backed up together.

The service listens on TCP 8443. Use loopback/SSH forwarding or an HTTPS reverse
proxy for remote access. The image contains no user data or credentials.

Project: https://github.com/jesongit/gamer
