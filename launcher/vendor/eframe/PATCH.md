# Gamer hidden startup patch

Source: crates.io `eframe` 0.36.2, upstream commit
`49682f8baa058bf49e011035cfbd6e825f88a5ef`, `crates/eframe`.
Retains upstream MIT OR Apache-2.0 licenses.

Only behavior change: `src/native/epi_integration.rs` initializes
`is_first_frame` from `native_options.viewport.visible.unwrap_or(true)` instead
of always `true`. This prevents `post_rendering` from forcibly showing a root
window explicitly created hidden. The app still reveals it with `Visible(true)`.
Ordinary initially visible applications keep the upstream first-paint behavior.

Why vendored: eframe 0.36.2 exposes no switch for its forced first-paint show;
hiding again from app logic happens too late and produces a visible flash.
Do not modify the shared Cargo registry cache. Remove this patch once an upstream
release honors the initial hidden flag; rerun the probe before switching back.

Windows verification (no server, ADB or network):
`cargo run --manifest-path launcher/Cargo.toml --example hidden_startup_probe`.
The probe paints a real native window and asserts that it remains hidden.

The crate's generated lockfile and Cargo cache marker are omitted; the launcher's
`Cargo.lock` remains the dependency lock.
