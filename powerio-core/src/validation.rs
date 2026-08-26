use std::path::Path;

pub(crate) const MAX_IDENTIFIER_BYTES: usize = 65_536;
pub(crate) const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 16_384;
pub(crate) const MAX_ARTIFACT_PATH_BYTES: usize = 4_096;
pub(crate) const MAX_ARTIFACT_SEGMENT_BYTES: usize = 255;
pub(crate) const MAX_FORMAT_ID_BYTES: usize = 127;
pub(crate) const MAX_DIAGNOSTIC_CODE_BYTES: usize = 255;

pub(crate) fn valid_nonempty_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_IDENTIFIER_BYTES && !value.contains('\0')
}

pub(crate) fn sanitize_message(message: impl Into<String>) -> String {
    let message = message.into();
    let single_line = message
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    truncate_utf8(single_line, MAX_DIAGNOSTIC_MESSAGE_BYTES)
}

fn truncate_utf8(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let suffix = "…";
    let mut end = limit.saturating_sub(suffix.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push_str(suffix);
    value
}

pub(crate) fn valid_rfc6901_pointer(pointer: &str) -> bool {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return false;
    }
    let bytes = pointer.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'~'
            && (index + 1 == bytes.len() || !matches!(bytes[index + 1], b'0' | b'1'))
        {
            return false;
        }
        index += 1;
    }
    true
}

pub(crate) fn path_exists_without_following(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_sanitization_is_one_line_and_utf8_safe() {
        let message = format!("first\n{}", "é".repeat(MAX_DIAGNOSTIC_MESSAGE_BYTES));
        let message = sanitize_message(message);
        assert!(message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES);
        assert!(!message.contains(['\n', '\r']));
        assert!(message.ends_with('…'));
    }

    #[test]
    fn pointer_validation_checks_escape_sequences() {
        assert!(valid_rfc6901_pointer(""));
        assert!(valid_rfc6901_pointer("/a~1b/~0value"));
        assert!(!valid_rfc6901_pointer("a"));
        assert!(!valid_rfc6901_pointer("/~2"));
        assert!(!valid_rfc6901_pointer("/~"));
    }
}
