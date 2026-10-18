# prod HTTP login policy — local validation, 2026-10-19

## Result

Production mode can explicitly use HTTP login cookies independently of its
credential/configuration rules. `GAMER_COOKIE_SECURE=false` overrides
`[auth].cookie_secure = true`; setting the environment variable to true forces
Secure cookies. File configuration applies when the variable is absent.
Unspecified policy retains the existing prod=true/dev=false defaults.

The Docker configuration template explicitly selects false for its HTTP entry
point. `restore.sh` forwards an explicitly supplied policy when creating a
container. Login and logout use the same policy; HttpOnly, SameSite=Strict,
password verification, session lifetime/revocation and same-origin checks remain.
Saving other system settings preserves the startup Cookie policy.

## Verification

Default features including wasm-runtime, Windows native test binary:

| Test group | Passed | Failed |
| --- | ---: | ---: |
| `api::auth::` | 28 | 0 |
| `api::tests::sec_tests::auth_tests::` | 23 | 0 |
| `config::tests::` | 21 | 0 |
| `settings::tests::` | 7 | 0 |

The HTTP integration case logs in with matching LAN Host/Origin, accesses
session/extensions/system-info/update APIs successfully, rejects missing login
with 401 and foreign Origin with 403, then revokes the session on logout.
The targeted `cookie` batch also passed all 11 tests; those overlap the table.
Raw group logs: `server/target/http-cookie-policy-20261019/*.log` (ignored).

Linux helper shell tests: `tools/test_gamer_restore_start.py`, 5 passed, including
both true and false environment forwarding and existing data/container guards.

`cargo fmt --manifest-path server/Cargo.toml --all -- --check` passed.
`cargo clippy --manifest-path server/Cargo.toml --bin gamer-server --tests -- -D warnings` passed.
`git diff --check` passed. Native builds used CARGO_BUILD_JOBS=1 and tests used
ADB_MDNS=0; Docker helpers used 512 MiB with no swap.

## Deployment boundary

At completion of these source-only checks the change had not yet been released.
`posase/gamer:v1.0.1` does not support the new policy. No remote production
container was changed, and no task/credential/browser data was reset.
The original local production container remains stopped. Image publication and
packaged-runtime validation are recorded separately in the v1.0.2 release evidence;
destination HTTP/account/scheduled-run acceptance remains a separate step.
