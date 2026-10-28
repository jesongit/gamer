# Gamer

Gamer provides browser cloud gaming, YAML automation and scheduled tasks. The
image includes the server, static web UI, Chromium, FFmpeg, ADB and local English /
Simplified Chinese OCR. A separate browser or VNC container is not required.

## Release

```sh
docker pull --platform linux/amd64 posase/gamer:v1.0.6
```

Docker release **v1.0.6** packages application **0.5.3**. Plugin and configuration
pages now offer a source dropdown for GitHub, LAN Gitea / Forgejo and custom URL
presets. Presets are shared on the server; each browser remembers its own choice.
Catalogs and archives are fetched by the server, with fixed size and SHA-256
checks, so LAN downloads do not depend on browser CORS. Public sources use HTTPS;
private or loopback IPs may use HTTP. A Git repository needs actual Release assets.
The official companion catalog provides YAML 0.3.26, Live 0.2.19 and Notify 0.1.2;
install or update plugins separately through the plugin page. Configuration
packages are separate; the latest HSR package is 1.0.2. Database schema remains 7.
The `latest` image tag follows this stable release; versioned tags stay immutable.
The published architecture is **linux/amd64**, for Intel/AMD x86_64 hosts.
The previous `v1.0`, `v1.0.1`, `v1.0.2`, `v1.0.3`, `v1.0.4` and `v1.0.5` tags remain unchanged. No ARM64 release is claimed.

The v1.0.6 deployment bundle includes the unchanged Chromium seccomp file:
https://github.com/jesongit/gamer/releases/tag/docker-v1.0.6

## Fresh installation with docker run

Download and extract the deployment bundle from the release above. It includes
`deploy/docker/chromium-seccomp.json` and requires no Compose.

```sh
docker volume create gamer-production-data
docker run --rm -it --memory 512m --memory-swap 512m --entrypoint python3 \
  --mount type=volume,src=gamer-production-data,dst=/data \
  posase/gamer:v1.0.6 /app/tools/gamer_docker.py init

docker run -d --name gamer --init --restart unless-stopped \
  --stop-timeout 90 --shm-size 1g \
  --security-opt seccomp=./deploy/docker/chromium-seccomp.json \
  -p 18443:8443 -e TZ=Asia/Shanghai \
  -e GAMER_PROFILE=prod -e GAMER_COOKIE_SECURE=false \
  --mount type=volume,src=gamer-production-data,dst=/data \
  --log-driver json-file --log-opt max-size=10m --log-opt max-file=5 \
  posase/gamer:v1.0.6
```

Open `http://localhost:18443` using your configured password. There is no default
password. New installations start with automatic scheduling disabled. Chromium
runs sandboxed as UID 1000; application self-update is off. Browser idle reclamation
is 30 minutes. CDP remains internal and must not be published.

For HTTP access use `GAMER_PROFILE=prod` with `GAMER_COOKIE_SECURE=false`.
For an HTTPS entrypoint set `GAMER_COOKIE_SECURE=true`. Password checks,
HttpOnly/SameSite cookies and same-origin checks remain enabled. This flag requires
v1.0.2 or later. Existing volumes retain their configuration and need an explicit
environment setting when switching from dev to prod with HTTP.

## Update an existing installation with docker run

Wait for active tasks to finish, then run from the original deployment directory.
This example uses the existing container name `gamer`. It takes a private full cold
backup, retains the previous container, and inherits its exact volume and bind mounts.
Keep any additional custom ports or environment settings from your original command.

```bash
image=posase/gamer:v1.0.6
docker pull "$image" &&
docker run --rm --entrypoint cat "$image" /app/chromium-seccomp.json > gamer-seccomp-v106.json &&
docker stop -t 90 gamer &&
(umask 077; docker run --rm --memory 512m --memory-swap 512m \
  --volumes-from gamer:ro --entrypoint python3 "$image" \
  /app/tools/volume-transfer.py export --gzip > "gamer-before-v106-$(date +%s).tar.gz") &&
docker rename gamer gamer-before-v106 &&
docker update --restart=no gamer-before-v106 &&
docker run -d --name gamer --init --restart unless-stopped --stop-timeout 90 --shm-size 1g \
  --volumes-from gamer-before-v106 --security-opt "seccomp=$PWD/gamer-seccomp-v106.json" \
  -p 18443:8443 -e GAMER_PROFILE=prod -e GAMER_COOKIE_SECURE=false \
  --log-driver json-file --log-opt max-size=10m --log-opt max-file=5 "$image"
```

Database schema remains 7. The existing username/password, credential key, tasks,
task enabled flags and scheduler configuration stay in the original `/data` mount.
An existing installation does not need init, restore or activate again. If any step
fails, the `&&` chain stops; retain the backup and previous container for diagnosis.
Rollback after an upgrade should restore the matching cold backup into a fresh
volume and start the previous image, keeping the upgraded volume intact.

## Existing data migration

**Skip init when migrating existing data.** Restore the complete data context to
an empty named volume: database, tasks, packages, plugin installations, browser
profiles and the original credential key must remain together. The legacy v1.0.2 migration
bundle's `restore.sh` validates the data and starts Gamer with scheduling disabled.
Verify account login and the task list, stop the old production instance, then run
`bash restore.sh activate`. Only one machine should schedule the same accounts.
That legacy migration bundle targets v1.0.2; after migration, use the update
instructions above to switch the existing installation to v1.0.6.

Detailed steps: https://github.com/jesongit/gamer/blob/docker-v1.0.2/deploy/docker/MIGRATION.md

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
the previous image and matching pre-upgrade backup for rollback. This release has no
database migration. Its isolated upgrade and cold-backup rollback checks use
v1.0.5 with synthetic data; see the release receipt for the completed results. Data and credential keys must
be backed up together. Tests used local synthetic pages, not real game accounts.

The service listens on TCP 8443. Publish host TCP 18443 as above for direct HTTP,
or use SSH forwarding / an HTTPS reverse proxy. The image contains no user data
or credentials.

### Local live sources in Docker

The live plugin's video source and feedback page are local read-only URLs. For a
streaming application on the Docker host, publish the server only on host
loopback: `-p 127.0.0.1:18443:8443`. In `/data/config.toml`, add:

```toml
[local_read]
published_port = 18443
docker_gateway = "172.17.0.1"
```

The gateway above is an example. Pin the exact source IP observed inside the
container for a host connection; Docker networks can use different addresses.
Do not use this exception with a port published on all host interfaces, a public
proxy, or untrusted traffic. Restart the container after configuration changes,
then copy both source URLs again; their read keys are retained. Re-enable feedback
and video output after restart.

Without this configuration, only actual loopback peers may read plugin sources.
The explicit exception still requires a loopback Host at the published port, a
same-origin request when Origin is present, the plugin's separate read key, and
an enabled source. Forwarded headers are rejected. It grants no management or MCP
access, and does not change their real-peer authentication checks.

Project: https://github.com/jesongit/gamer
