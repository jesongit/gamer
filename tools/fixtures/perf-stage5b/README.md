# QA-007 perf-stage5b fixtures

`prepare_stress_data.py` has two explicit modes:

- `--mode real` writes materialized blob rows into the server-created SQLite
  schema-v1 database and requires `fsutil sparse queryflag` to prove the file
  is not sparse. This mode is the only mode allowed through the real launcher
  upgrade/snapshot path.
- `--mode sparse` creates a logical 1 GiB sparse file and 2048+ small files for
  preflight/fixture checks only. The stress runner records this as a substitute
  and skips the real copy; it is never a full-pass result.

`verify_snapshot.py` independently checks the snapshot file set, manifest
self-hash, every file size/SHA-256, and SQLite integrity. `gen_qa_manifests.py`
creates app-only signed manifests for the local rig.
