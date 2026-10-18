The Chromium seccomp profile is vendored from Microsoft Playwright:
https://github.com/microsoft/playwright/blob/main/utils/docker/seccomp_profile.json

It extends Docker's default seccomp profile to permit unprivileged Chromium
user namespaces (`clone`, `setns`, `unshare`). No Playwright runtime is installed.
The upstream Apache-2.0 license is included in LICENSE.chromium-seccomp.

Local changes (2026-10-19): allow `close_range` and `openat2` for runc's
post-seccomp file descriptor cleanup / safe procfs access. Return ENOSYS for
`clone3` so glibc can fall back to the existing `clone` rule. This follows the
corresponding rules in Moby's default profile; the remaining allowlist and
Chromium user namespace rules are unchanged. Seccomp remains enabled.

References:
- https://github.com/opencontainers/runc/issues/5007
- https://github.com/moby/profiles/blob/main/seccomp/default.json
