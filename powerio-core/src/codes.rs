//! Diagnostic identities emitted by the foundation crate.

crate::diagnostic_codes! {
    REQUEST_DIAGNOSTIC_INVALID_CODE = "REQUEST.DIAGNOSTIC.INVALID_CODE", Error,
        "a diagnostic code does not match the stable code grammar", category = Request;
    REQUEST_DIAGNOSTIC_MISSING_CATEGORY = "REQUEST.DIAGNOSTIC.MISSING_CATEGORY", Error,
        "an error was constructed from a registry entry without an error category", category = Request;
    REQUEST_RECORD_INVALID_IDENTIFIER = "REQUEST.RECORD.INVALID_IDENTIFIER", Error,
        "a common record identifier is empty, contains NUL, or exceeds its bound", category = Request;
    REQUEST_RECORD_INVALID_POINTER = "REQUEST.RECORD.INVALID_POINTER", Error,
        "a source map or diagnostic target is not an RFC 6901 pointer", category = Request;
    REQUEST_RECORD_INVALID_DIGEST = "REQUEST.RECORD.INVALID_DIGEST", Error,
        "a source digest is not valid for its declared algorithm", category = Request;
    REQUEST_RECORD_INVALID_SPAN = "REQUEST.RECORD.INVALID_SPAN", Error,
        "a source span is reversed or outside its declared source", category = Request;
    REQUEST_RECORD_DUPLICATE_ID = "REQUEST.RECORD.DUPLICATE_ID", Error,
        "a module record repeats an existing local identity", category = Request;
    REQUEST_RECORD_INVALID_EXTENSION = "REQUEST.RECORD.INVALID_EXTENSION", Error,
        "an extension key is not namespaced", category = Request;
    REQUEST_RECORD_TOO_LARGE = "REQUEST.RECORD.TOO_LARGE", Error,
        "a stored record exceeds a bound the constructors enforce", category = Request;
    REQUEST_RECORD_ALLOCATION_REFUSED = "REQUEST.RECORD.ALLOCATION_REFUSED", Error,
        "the record identity index cannot reserve memory for the stated count", category = Request;

    REQUEST_FORMAT_INVALID_ID = "REQUEST.FORMAT.INVALID_ID", Error,
        "a format identifier does not match the stable format grammar", category = Request;

    VALIDATE_COMPONENT_INVALID_ID = "VALIDATE.COMPONENT.INVALID_ID", Error,
        "a component identity has an empty, NUL containing, or oversized part", category = Data;

    REQUEST_SOURCE_INVALID_NAME = "REQUEST.SOURCE.INVALID_NAME", Error,
        "an in-memory source name is empty, contains NUL, or exceeds its bound", category = Request;
    REQUEST_SOURCE_INVALID_PATH = "REQUEST.SOURCE.INVALID_PATH", Error,
        "a source path is empty or does not name a regular file or directory", category = Request;
    REQUEST_SOURCE_SYMLINK_REFUSED = "REQUEST.SOURCE.SYMLINK_REFUSED", Error,
        "source acquisition refused a symbolic link", category = Request;
    REQUEST_SOURCE_NOT_A_FILE = "REQUEST.SOURCE.NOT_A_FILE", Error,
        "source acquisition refused an entry that is not a regular file", category = Request;
    REQUEST_SOURCE_DIRECTORY_REQUIRED = "REQUEST.SOURCE.DIRECTORY_REQUIRED", Error,
        "a named child buffer was requested from a single-buffer source", category = Request;
    REQUEST_SOURCE_ESCAPES_ROOT = "REQUEST.SOURCE.ESCAPES_ROOT", Error,
        "a referenced file resolves outside the acquisition root", category = Request;
    REQUEST_SOURCE_UNKNOWN_BUFFER = "REQUEST.SOURCE.UNKNOWN_BUFFER", Error,
        "a referenced buffer was not supplied to an in-memory source", category = Request;

    READ_IO_METADATA = "READ.IO.METADATA", Error,
        "source metadata could not be read", category = Io;
    READ_IO_OPEN = "READ.IO.OPEN", Error,
        "a source buffer could not be opened", category = Io;
    READ_IO_READ = "READ.IO.READ", Error,
        "a source buffer could not be read completely", category = Io;
    READ_IO_SOURCE_CHANGED = "READ.IO.SOURCE_CHANGED", Error,
        "a source buffer changed while it was being acquired", category = Io;
    READ_IO_ALLOCATION_REFUSED = "READ.IO.ALLOCATION_REFUSED", Error,
        "memory for a source buffer could not be reserved", category = Io;
    READ_IO_REFERENCE_BUDGET = "READ.IO.REFERENCE_BUDGET", Error,
        "acquiring another referenced file would pass the source acquisition budget", category = Io;

    VALIDATE_TIME_POINT_INVALID_LABEL = "VALIDATE.TIME_POINT.INVALID_LABEL", Error,
        "a time point label is empty, contains NUL, or exceeds its bound", category = Data;
    VALIDATE_TIME_POINT_INVALID_DURATION = "VALIDATE.TIME_POINT.INVALID_DURATION", Error,
        "a time point duration has an invalid nanosecond remainder", category = Data;
    VALIDATE_TIME_SERIES_SHAPE = "VALIDATE.TIME_SERIES.SHAPE", Error,
        "time point and value counts differ", category = Data;

    VALIDATE_SCENARIO_INVALID_ID = "VALIDATE.SCENARIO.INVALID_ID", Error,
        "a scenario identity is empty, contains NUL, or exceeds its bound", category = Data;
    VALIDATE_SCENARIO_DUPLICATE_ID = "VALIDATE.SCENARIO.DUPLICATE_ID", Error,
        "a scenario set repeats a case-sensitive scenario identity", category = Data;
    VALIDATE_SCENARIO_MISSING_PROBABILITY = "VALIDATE.SCENARIO.MISSING_PROBABILITY", Error,
        "a scenario set supplies probabilities for only some entries", category = Data;
    VALIDATE_SCENARIO_INVALID_PROBABILITY = "VALIDATE.SCENARIO.INVALID_PROBABILITY", Error,
        "a scenario probability is negative or nonfinite", category = Data;
    VALIDATE_SCENARIO_PROBABILITY_SUM = "VALIDATE.SCENARIO.PROBABILITY_SUM", Error,
        "scenario probabilities do not sum to one within the required tolerance", category = Data;
    VALIDATE_SCENARIO_ALLOCATION_REFUSED = "VALIDATE.SCENARIO.ALLOCATION_REFUSED", Error,
        "memory for scenario identity validation could not be reserved", category = Data;

    REQUEST_OUTPUT_INVALID_ARTIFACT_PATH = "REQUEST.OUTPUT.INVALID_ARTIFACT_PATH", Error,
        "an artifact path is not a portable nonempty relative path", category = Request;
    REQUEST_OUTPUT_INVALID_LAYOUT = "REQUEST.OUTPUT.INVALID_LAYOUT", Error,
        "an artifact inventory does not match the requested output layout", category = Request;
    REQUEST_OUTPUT_DUPLICATE_ARTIFACT = "REQUEST.OUTPUT.DUPLICATE_ARTIFACT", Error,
        "an artifact inventory repeats a path", category = Request;
    REQUEST_OUTPUT_COLLISION = "REQUEST.OUTPUT.COLLISION", Error,
        "an output target already exists", category = Request;
    EMIT_IO_STAGING = "EMIT.IO.STAGING", Error,
        "a sibling output staging path could not be created", category = Io;
    EMIT_IO_WRITE = "EMIT.IO.WRITE", Error,
        "an output artifact could not be written", category = Io;
    EMIT_IO_COMMIT = "EMIT.IO.COMMIT", Error,
        "a complete staged output could not be moved into place", category = Io;
    EMIT_IO_CLEANUP = "EMIT.IO.CLEANUP", Error,
        "an incomplete output staging path could not be removed", category = Io;
}

pub use ALL as CORE_DIAGNOSTIC_CODES;

#[cfg(test)]
mod tests {
    #[test]
    fn core_registry_is_valid() {
        assert_eq!(
            crate::check_registry(super::ALL.iter().copied()),
            Vec::<String>::new()
        );
    }
}
