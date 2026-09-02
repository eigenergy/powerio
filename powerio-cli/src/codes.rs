//! Diagnostic identities the command line raises for failures of its own:
//! arguments the request cannot satisfy, an input that lacks what a command
//! needs, an output the command refuses to write, and a failure that reached
//! the top of the program without a registered code. Every failure the binary
//! reports is a `powerio_core::Error` built from a registered code, so the
//! exit status and the JSON rendering read the same category and record as a
//! library failure.

powerio_core::diagnostic_codes! {
    REQUEST_CLI_FORMAT_REQUIRED = "REQUEST.CLI.FORMAT_REQUIRED", Error,
        "a case on standard input needs a declared --from format", category = Request;
    REQUEST_CLI_TARGET_UNSUPPORTED = "REQUEST.CLI.TARGET_UNSUPPORTED", Error,
        "the command cannot write the requested target format", category = Request;
    REQUEST_CLI_OUTPUT_REQUIRED = "REQUEST.CLI.OUTPUT_REQUIRED", Error,
        "the requested target format needs an output path", category = Request;
    REQUEST_CLI_FAMILY_MISMATCH = "REQUEST.CLI.FAMILY_MISMATCH", Error,
        "no conversion joins the input and the requested target", category = Request;
    REQUEST_CLI_OPTION_INVALID = "REQUEST.CLI.OPTION_INVALID", Error,
        "an option value is not valid for the command", category = Request;
    REQUEST_CLI_NO_CASES = "REQUEST.CLI.NO_CASES", Error,
        "a batch input holds no case files", category = Request;
    PARSE_CLI_ERRORS_REPORTED = "PARSE.CLI.ERRORS_REPORTED", Error,
        "the reader reported errors, so the output is incomplete", category = Parse;
    VALIDATE_CLI_INPUT_LACKS_DATA = "VALIDATE.CLI.INPUT_LACKS_DATA", Error,
        "the input carries none of the data the command needs", category = Data;
    EMIT_CLI_SIDECAR_PATH = "EMIT.CLI.SIDECAR_PATH", Error,
        "an emitted sidecar path is not relative to the output directory", category = Output;
    BIND_CLI_UNCLASSIFIED = "BIND.CLI.UNCLASSIFIED", Error,
        "the command failed without a registered diagnostic code";
}

#[cfg(test)]
mod tests {
    use powerio_core::check_registry;

    #[test]
    fn every_command_line_code_is_well_formed_and_registered_once() {
        let problems = check_registry(super::ALL.iter().copied());
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
