#!/usr/bin/env bash
# Run from an extracted migration bundle on the destination Linux host.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

image="${GAMER_IMAGE:-posase/gamer:v1.0.1}"
volume='gamer-production-data'
container='gamer'
mode="${1:-restore}"
case "$mode" in restore|activate) ;; *) echo 'Usage: bash restore.sh [restore|activate]'; exit 2;; esac

docker info >/dev/null
if [[ "$mode" == restore ]]; then
  archive='gamer-data.tar.gz'
  [[ -s "$archive" ]] || archive='gamer-data.tar'
  test -s "$archive" || { echo 'Upload gamer-data.tar.gz into this directory first'; exit 1; }
  test -s deploy/docker/chromium-seccomp.json
  if docker container inspect "$container" >/dev/null 2>&1; then
    echo 'Container gamer already exists; nothing was changed. Choose a fresh destination.'
    exit 1
  fi
  if [[ -n "$(docker ps --filter "volume=$volume" --format '{{.ID}}')" ]]; then
    echo 'A container is using the destination volume; stop it before restoring'
    exit 1
  fi
  docker pull --platform linux/amd64 "$image"
  docker volume create "$volume" >/dev/null
else
  # Do not enable scheduling or restart a container belonging to another install.
  expected_id="$(docker image inspect "$image" --format '{{.Id}}')"
  actual_id="$(docker inspect "$container" --format '{{.Image}}')"
  mounted="$(docker inspect "$container" --format '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Name}}{{end}}{{end}}')"
  [[ "$actual_id" == "$expected_id" && "$mounted" == "$volume" ]] || {
    echo 'Container image or data volume differs from this migration; nothing was changed'
    exit 1
  }
  # Work offline, as with the original backup. No scheduler/config write while live.
  docker stop --time 90 "$container" >/dev/null
fi

docker run --rm --memory 512m --memory-swap 512m --user 0 --entrypoint python3 \
  --mount "type=volume,src=$volume,dst=/data" \
  --mount "type=bind,src=$PWD,dst=/migration,readonly" \
  "$image" /migration/deploy/docker/restore-volume.py "$mode" \
  --archive "/migration/${archive:-gamer-data.tar.gz}"

# The released bundle pins this image to the published digest.
if [[ "$mode" == restore ]]; then
  docker run -d --name "$container" --platform linux/amd64 \
    --init --restart unless-stopped --stop-timeout 90 --shm-size 1g \
    --security-opt "seccomp=$PWD/deploy/docker/chromium-seccomp.json" \
    -p 127.0.0.1:18443:8443 \
    -e TZ=Asia/Shanghai -e RUST_LOG=info \
    -e TOKIO_WORKER_THREADS=2 -e MALLOC_ARENA_MAX=2 \
    --mount "type=volume,src=$volume,dst=/data" \
    --log-driver json-file --log-opt max-size=10m --log-opt max-file=5 \
    "$image"
else
  docker start "$container" >/dev/null
fi
docker ps --filter "name=^/${container}$" --format 'table {{.Names}}\t{{.Status}}\t{{.Ports}}'
