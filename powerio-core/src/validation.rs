pub(crate) const MAX_IDENTIFIER_BYTES: usize = 65_536;
pub(crate) const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 16_384;
/// Raw bytes retained for one message while decoding, before sanitization.
/// Every writer sanitizes at construction, so a stored message near this bound
/// was not produced by PowerIO; past it the raw text is truncated, and the
/// result still passes through [`sanitize_message`].
pub(crate) const MAX_DIAGNOSTIC_MESSAGE_DECODE_BYTES: usize = 4 * MAX_DIAGNOSTIC_MESSAGE_BYTES;
pub(crate) const MAX_ARTIFACT_PATH_BYTES: usize = 4_096;
pub(crate) const MAX_ARTIFACT_SEGMENT_BYTES: usize = 255;
pub(crate) const MAX_FORMAT_ID_BYTES: usize = 127;
pub(crate) const MAX_DIAGNOSTIC_CODE_BYTES: usize = 255;
pub(crate) const MAX_DIAGNOSTIC_TARGET_BYTES: usize = 8_192;
pub(crate) const MAX_DIAGNOSTIC_SPANS: usize = 256;
pub(crate) const MAX_DIAGNOSTIC_RELATED: usize = 256;
pub(crate) const MAX_DIAGNOSTIC_DETAIL_KEYS: usize = 256;
pub(crate) const MAX_SOURCE_MAP_SPANS: usize = 256;
pub(crate) const MAX_HISTORY_PARAMETERS: usize = 256;
pub(crate) const MAX_HISTORY_NOTES: usize = 256;
/// Module level record counts. Each stored module list is refused at its
/// count while it is decoded, so a small hostile document cannot declare its
/// way into an unbounded record allocation.
pub const MAX_MODULE_SOURCES: usize = 262_144;
pub const MAX_MODULE_SOURCE_MAP_ENTRIES: usize = 262_144;
pub const MAX_MODULE_DIAGNOSTICS: usize = 262_144;
pub const MAX_MODULE_HISTORY_ENTRIES: usize = 65_536;

/// A locator identifies an element, so it is bounded but never shortened: a
/// truncated RFC 6901 pointer names a different element, or none.
pub(crate) fn valid_diagnostic_target(target: &str) -> bool {
    !target.is_empty() && target.len() <= MAX_DIAGNOSTIC_TARGET_BYTES && !target.contains('\0')
}

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
