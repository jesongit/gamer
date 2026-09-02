/// Validate a portable single path component used for packages and resource files.
pub(crate) fn safe_name(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.starts_with('.')
        || value.ends_with('.')
        || is_windows_reserved_name(value)
    {
        return None;
    }
    if value
        .chars()
        .any(|ch| !(ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_')))
    {
        return None;
    }
    Some(value.to_string())
}

/// Windows interprets these basenames as device files, even when an extension is present.
pub(crate) fn is_windows_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}
