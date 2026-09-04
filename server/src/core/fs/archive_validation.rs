/// Shared archive limits for resource importers (App Package install).
pub const IMPORT_MAX_ARCHIVE_BYTES: usize = 20 * 1024 * 1024;
pub const IMPORT_MAX_TOTAL_BYTES: usize = 100 * 1024 * 1024;
pub const IMPORT_MAX_ENTRIES: usize = 500;
/// 单个脚本/函数库 YAML 上限（保存与解析共用）
pub const IMPORT_MAX_YAML_BYTES: usize = 1024 * 1024;
