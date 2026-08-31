//! 错误码常量：与 release/contracts/validate-manifest.mjs / manifest-v1.md 保持一致。

pub const IO_ERROR: &str = "io-error";
pub const UNSIGNED_MANIFEST: &str = "unsigned-manifest";
pub const SIG_FORMAT_INVALID: &str = "sig-format-invalid";
pub const UNKNOWN_KEY_ID: &str = "unknown-key-id";
pub const SIGNATURE_INVALID: &str = "signature-invalid";
pub const JSON_PARSE_FAILED: &str = "json-parse-failed";
pub const SCHEMA_INVALID: &str = "schema-invalid";

pub const UNKNOWN_SCHEMA_VERSION: &str = "unknown-schema-version";
pub const PRODUCT_MISMATCH: &str = "product-mismatch";
pub const VERSION_NOT_SEMVER: &str = "version-not-semver";
pub const CHANNEL_INVALID: &str = "channel-invalid";
pub const VERSION_DOWNGRADE: &str = "version-downgrade";
pub const CHANNEL_MISMATCH: &str = "channel-mismatch";
pub const UNKNOWN_PLATFORM: &str = "unknown-platform";
pub const JAR_BINDING_MISMATCH: &str = "jar-binding-mismatch";
pub const JAR_PATH_NOT_ASSETS: &str = "jar-path-not-assets";
pub const SHA256_UPPERCASE: &str = "sha256-uppercase";
pub const SHA256_WRONG_LENGTH: &str = "sha256-wrong-length";
pub const SIZE_NEGATIVE: &str = "size-negative";
pub const SIZE_OVERSIZED: &str = "size-oversized";
pub const LAUNCHER_TOO_OLD: &str = "launcher-too-old";

pub const PATH_EMPTY: &str = "path-empty";
pub const PATH_BACKSLASH: &str = "path-backslash";
pub const PATH_ABSOLUTE: &str = "path-absolute";
pub const PATH_DRIVE_LETTER: &str = "path-drive-letter";
pub const PATH_ADS_COLON: &str = "path-ads-colon";
pub const PATH_ILLEGAL_CHARS: &str = "path-illegal-chars";
pub const PATH_NOT_NORMALIZED: &str = "path-not-normalized";
pub const PATH_DOTDOT: &str = "path-dotdot";
pub const PATH_TRAILING_DOT_SPACE: &str = "path-trailing-dot-space";
pub const PATH_RESERVED_NAME: &str = "path-reserved-name";
pub const PATH_DUPLICATE_ENTRY: &str = "path-duplicate-entry";
pub const PATH_CASE_COLLISION: &str = "path-case-collision";
