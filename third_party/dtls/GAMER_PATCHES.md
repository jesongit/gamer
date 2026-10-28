# DTLS dependency security patch

This is the published `dtls` 0.17.1 source from the final Tokio WebRTC line.
Upstream repository revision: `e9fe1bc4207a0c6b8d28d5583a118419be60f3c2`.
Source archive: https://crates.io/api/v1/crates/dtls/0.17.1/download
Archive SHA256: `2f016db07b91e9d79cc60a152c163d3f0ce2d4c0173cb3964de3526aab6e07fa`.
Both upstream licenses are retained next to this file.

Upstream 0.17.1 depends on P-256/P-384 0.13 and therefore vulnerable
crypto-bigint 0.5. Gamer uses RustCrypto P-256/P-384 0.14, which resolves the
fixed crypto-bigint 0.7 line, and adapts its public point and secure random
generation APIs. The DTLS protocol implementation and curve negotiation are
retained. No custom cryptographic arithmetic is introduced.

See RUSTSEC-2026-0387: https://rustsec.org/advisories/RUSTSEC-2026-0387.html
The upstream library lockfile is omitted because the Gamer application lockfile
owns the resolved dependency graph. This patch should be removed when a
compatible upstream release adopts the repaired RustCrypto dependencies.
