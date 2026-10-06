# AI automation local acceptance results

Verified on 2026-10-05T21:30:05Z: **23/23 HTTP acceptance checks passed**.
The summed check duration was 13.601 seconds.
The isolated server and deterministic provider both stopped cleanly.

This is the final host 0.3.0 build5 working-tree snapshot, with YAML 0.2.0,
video 0.2.0 and AI 0.4.0. These are local test artifacts; no release was published.
The source baselines were main `ad402a40f1883314d7f0ff7ed03502ec8cb0cbf9`
and plugins `913ccf8b6bd1f3029a57cbc6e8d4f16782e9671e`, with the current
implementation changes applied. This does not claim those old commits alone
contain the implementation.

## Verified behavior

- Real server, authentication, current-plugin installation and production routes
- Real MP4 import/frame extraction and portable sample creation, with and without
  original clips; tampered, renamed-ID and JSON-text fake archives rejected
- Actual shared parser/interpreter/native functions/NCC matcher, using unchanged
  production 300ms before/after-click delays and real evidence through 1000ms
- A/C confirmation-present and B confirmation-absent paths pass reproducibly;
  incorrect actions, missing/wrong templates, missing finish, mandatory-confirm
  B failure, and a negative END after earlier success cannot become validated saves
- Original source media is removed before clip-backed validation. Replay still
  observes an unlisted archived-video frame at 700000us, retains source identity,
  and repeats deterministically. A correctly rehashed PNG contradicting the
  clip is rejected with `EVIDENCE_ANCHOR_PIXELS`
- Candidate revision conflict, real settings change, concurrent resource change,
  atomic script/template save and joint rollback checks pass
- Local Responses protocol probe, generated PNG and clip-backed candidates,
  two bounded failed-proposal requests, 2048-token pre-request budget rejection,
  and attempted selected-sample deletion rejection pass
- Cancellation after submission preserves unknown usage, blocks another automatic
  model request, and permits manual validation. Context/configuration/plugin
  changes cannot apply a late result to another package
- Held-out synthetic D passes with the exact generated YAML and byte-identical
  template bytes, without being sent to the provider. Crop-source A is retained
  only for the existing template-provenance API. D is reported separately and
  original selected A/B/C remain unchanged
- Trace HTTP authentication and missing-evidence/retention responses pass

## Artifact identity

- Executed server SHA-256: `4352a41a19df1763ffa0c393b59ef5fffbea56330cf97473dc3d850bd77c6188`
- YAML runtime-contract 2 component SHA-256:
  `405da33bb7b242425d997e55fa5c853dcac14a90a3ba8a5fc957e225a80a2893`
- All three packaged plugin JavaScript/CSS sets match their final current `dist/ui`
  hashes, rechecked after the final aggregate UI build. Full archive, UI and
  harness hashes are in the machine-readable result

[Machine-readable results](ai-automation-local-results.json) contain every check,
duration, relevant diagnostic and artifact hash. The test secrets are not included.
[Runner documentation](ai-automation-local.md) describes prerequisites, fixtures,
flags and reproduction. This run used `tools/verify-ai-automation-local.py` with
the final prebuilt server and freshly componentized guest, without skip flags.

## Deliberately unverified

The provider is a deterministic loopback protocol test double. The held-out result
is synthetic replay coverage, **not evidence of real-model generalization**.
Real model quality/cost, real Android/CDP recording alignment, actual game control,
and live-target Trace image capture remain deferred.

Supported-browser UI smoke was attempted separately; navigation to loopback was
blocked with `net::ERR_BLOCKED_BY_CLIENT`. No browser bypass was attempted and
browser visual acceptance is not claimed. UI component tests and broader Rust
suite results are tracked separately by the integration report.
