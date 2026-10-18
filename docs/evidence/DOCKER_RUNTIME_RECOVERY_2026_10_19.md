# Docker v1.0.1 migration runtime recovery

The destination reported `restored_tasks: 4` and `scheduler_enabled: false`,
then OCI startup failed in runc's post-seccomp descriptor cleanup with
`unsafe procfs detected ... openat2 ... function not implemented`.
The restored volume must be reused, not extracted over or deleted.

The vendored Playwright seccomp profile omitted `close_range` and `openat2`.
The local negative control confirmed both calls returned ENOSYS with that
profile. The corrected profile permits those two calls and explicitly returns
ENOSYS for `clone3`, preserving glibc's fallback to the existing clone rule.
All other rules and the default deny action remain unchanged.

`bash restore.sh start` now validates the existing volume read-only, requires
automatic scheduling to remain disabled, checks container image/volume identity
and rejects live consumers. It tests the runtime profile before proceeding,
disables restart on the stopped failed container and renames it for retention,
then creates Gamer using the unchanged `posase/gamer:v1.0.1` image and the same
data volume. No extraction, credential regeneration or automatic activation occurs.

Local validation on Docker Desktop Linux amd64, runc 1.5.1:

- Restore helper: 5 passed, including read-only recovery and missing-key rejection.
- Recovery shell fake-Docker boundaries: 4 passed, including foreign/live/busy
  rejection and preflight/verification failure before container mutation.
- Production archive selection/data preservation: 3 passed.
- Actual corrected-profile syscall/thread/child-process checks: 4 passed.
- Old-profile negative control: both close_range and openat2 checks failed with
  errno 38; the clone3 fallback and thread checks passed.
- Chromium headless sandbox DOM smoke: passed, no --no-sandbox, no account data.
- Shell syntax and git diff whitespace checks: passed.

Docker helpers were sequential and capped at 512 MiB without swap. The original
production container remains stopped, original data and backups are preserved.
No image build or Docker Hub update occurred. Destination execution and real
account login / scheduled-run acceptance remain pending user verification.

Upstream explanation:
https://github.com/opencontainers/runc/issues/5007
and current default syscall rules:
https://github.com/moby/profiles/blob/main/seccomp/default.json
