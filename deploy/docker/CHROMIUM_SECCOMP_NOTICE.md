The Chromium seccomp profile is vendored from Microsoft Playwright:
https://github.com/microsoft/playwright/blob/main/utils/docker/seccomp_profile.json

It extends Docker's default seccomp profile to permit unprivileged Chromium
user namespaces (`clone`, `setns`, `unshare`). No Playwright runtime is installed.
The upstream Apache-2.0 license is included in LICENSE.chromium-seccomp.
