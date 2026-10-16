//! Loss-preserving support for PSS/E transient-stability DYR records.
//!
//! A DYR file is a model library paired with a steady-state case, not itself a
//! power-flow network. This module therefore keeps dynamic records separate
//! from BalancedNetwork. Model-specific parameter schemas are intentionally
//! not guessed: unknown/user-defined model names and their ordered parameters
//! remain available to callers.
//!
//! Parsing is bounded and quote-aware. The original source is retained, so
//! DynamicCase::to_dyr_string reproduces comments, whitespace, line endings,
//! token spelling, and record order exactly. No model is silently discarded.

use std::collections::BTreeSet;
use std::fmt;

const MAX_DYR_BYTES: usize = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;
const MAX_FIELDS_PER_RECORD: usize = 4_096;
const MAX_FIELD_BYTES: usize = 64 * 1024;

/// Parsed PSS/E dynamic-model data, kept separate from a steady-state network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicCase {
    source: String,
    records: Vec<DynamicRecord>,
}

impl DynamicCase {
    /// Dynamic model records in source order.
    #[must_use]
    pub fn records(&self) -> &[DynamicRecord] {
        &self.records
    }

    /// Return the exact source text used to construct this case.
    ///
    /// The records are read-only, so returning the retained source cannot
    /// accidentally emit stale data after an edit.
    #[must_use]
    pub fn to_dyr_string(&self) -> &str {
        &self.source
    }

    /// Find references to buses absent from the supplied static-case bus IDs.
    ///
    /// The check deliberately validates only the generic bus reference. It
    /// does not assume that every model's second identifier is a generator ID:
    /// DYR model families use different device keys, and custom models are
    /// retained without pretending to understand their schemas.
    #[must_use]
    pub fn unresolved_buses(&self, bus_ids: &BTreeSet<u32>) -> Vec<UnresolvedBusReference> {
        self.records
            .iter()
            .filter_map(|record| {
                let bus_id = record.bus_id;
                (!bus_ids.contains(&bus_id)).then(|| UnresolvedBusReference {
                    line: record.line,
                    model_name: record.model_name.clone(),
                    bus_id,
                })
            })
            .collect()
    }
}

/// One DYR model record. Parameters remain ordered and lexically decoded;
/// model-specific interpretation belongs in a separate, schema-backed layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicRecord {
    model_name: String,
    bus_id: u32,
    device_id: Option<String>,
    parameters: Vec<String>,
    line: usize,
    raw_record: String,
}

impl DynamicRecord {
    /// Raw model name as represented by the record, without surrounding quotes.
    #[must_use]
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Bus identifier in the first DYR field.
    #[must_use]
    pub fn bus_id(&self) -> u32 {
        self.bus_id
    }

    /// Optional second device-key field, commonly a generator/load identifier.
    #[must_use]
    pub fn device_id(&self) -> Option<&str> {
        self.device_id.as_deref()
    }

    /// Ordered parameter tokens after the generic bus/model/device fields.
    #[must_use]
    pub fn parameters(&self) -> &[String] {
        &self.parameters
    }

    /// One-based source line where the record starts.
    #[must_use]
    pub fn line(&self) -> usize {
        self.line
    }

    /// The record's source spelling, excluding the terminating slash.
    #[must_use]
    pub fn raw_record(&self) -> &str {
        &self.raw_record
    }
}

/// A DYR record whose bus reference is not present in a static case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnresolvedBusReference {
    pub line: usize,
    pub model_name: String,
    pub bus_id: u32,
}

/// Errors detected while reading a DYR source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DyrError {
    InputTooLarge { actual: usize, maximum: usize },
    InvalidUtf8,
    UnterminatedQuote { line: usize },
    UnterminatedRecord { line: usize },
    MissingModelName { line: usize },
    InvalidBusId { line: usize, token: String },
    TooManyRecords { maximum: usize },
    TooManyFields { line: usize, maximum: usize },
    FieldTooLong { line: usize, maximum: usize },
}

impl fmt::Display for DyrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge { actual, maximum } => {
                write!(f, "DYR input is {actual} bytes; maximum is {maximum}")
            }
            Self::InvalidUtf8 => f.write_str("DYR input is not valid UTF-8"),
            Self::UnterminatedQuote { line } => {
                write!(f, "unterminated quoted field starting at line {line}")
            }
            Self::UnterminatedRecord { line } => {
                write!(
                    f,
                    "DYR record starting at line {line} has no terminating slash"
                )
            }
            Self::MissingModelName { line } => {
                write!(f, "DYR record at line {line} has no model name")
            }
            Self::InvalidBusId { line, token } => {
                write!(f, "invalid DYR bus identifier {token:?} at line {line}")
            }
            Self::TooManyRecords { maximum } => {
                write!(f, "DYR input exceeds the maximum of {maximum} records")
            }
            Self::TooManyFields { line, maximum } => {
                write!(f, "DYR record at line {line} exceeds {maximum} fields")
            }
            Self::FieldTooLong { line, maximum } => {
                write!(f, "DYR field at line {line} exceeds {maximum} bytes")
            }
        }
    }
}

impl std::error::Error for DyrError {}

/// Parse PSS/E DYR text without assuming a closed list of model names.
///
/// Records are delimited by slash outside quoted strings. Exclamation mark
/// starts a line-comment outside quotes. Fields are comma-separated; single-
/// quoted values may contain commas or slashes, and doubled single quotes
/// represent one literal quote. Parameter tokens remain strings because their
/// types and meanings depend on the model schema.
pub fn parse_dyr(text: &str) -> Result<DynamicCase, DyrError> {
    if text.len() > MAX_DYR_BYTES {
        return Err(DyrError::InputTooLarge {
            actual: text.len(),
            maximum: MAX_DYR_BYTES,
        });
    }

    let uncommented = strip_comments(text)?;
    let mut records = Vec::new();
    let mut start = 0;
    let mut start_line = 1;
    let mut line = 1;
    let mut in_quote = false;
    let bytes = uncommented.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                if uncommented[start..index].trim().is_empty() {
                    start_line = line + 1;
                }
                line += 1;
                index += 1;
            }
            b'\'' => {
                if in_quote && index + 1 < bytes.len() && bytes[index + 1] == b'\'' {
                    index += 2;
                } else {
                    in_quote = !in_quote;
                    index += 1;
                }
            }
            b'/' if !in_quote => {
                let raw = uncommented[start..index].trim();
                if !raw.is_empty() {
                    if records.len() >= MAX_RECORDS {
                        return Err(DyrError::TooManyRecords {
                            maximum: MAX_RECORDS,
                        });
                    }
                    records.push(parse_record(raw, start_line)?);
                }
                start = index + 1;
                start_line = line;
                index += 1;
            }
            _ => index += 1,
        }
    }

    let trailing = &uncommented[start..];
    if !trailing.trim().is_empty() {
        return Err(DyrError::UnterminatedRecord { line: start_line });
    }

    Ok(DynamicCase {
        source: text.to_owned(),
        records,
    })
}

/// Parse DYR bytes, rejecting invalid UTF-8 rather than replacing bytes.
pub fn parse_dyr_bytes(bytes: &[u8]) -> Result<DynamicCase, DyrError> {
    if bytes.len() > MAX_DYR_BYTES {
        return Err(DyrError::InputTooLarge {
            actual: bytes.len(),
            maximum: MAX_DYR_BYTES,
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| DyrError::InvalidUtf8)?;
    parse_dyr(text)
}

fn strip_comments(text: &str) -> Result<String, DyrError> {
    let mut output = String::with_capacity(text.len());
    let mut in_quote = false;
    let mut in_comment = false;
    let mut line = 1;
    let mut quote_start_line = 1;
    let mut chars = text.chars().peekable();

    while let Some(character) = chars.next() {
        if character == '\n' {
            line += 1;
            in_comment = false;
            output.push('\n');
            continue;
        }
        if in_comment {
            output.push(' ');
            continue;
        }
        if character == '\'' {
            output.push(character);
            if in_quote && chars.peek() == Some(&'\'') {
                output.push(chars.next().unwrap_or('\''));
                continue;
            }
            if !in_quote {
                quote_start_line = line;
            }
            in_quote = !in_quote;
            continue;
        }
        if character == '!' && !in_quote {
            in_comment = true;
            output.push(' ');
            continue;
        }
        output.push(character);
    }

    if in_quote {
        return Err(DyrError::UnterminatedQuote {
            line: quote_start_line,
        });
    }
    Ok(output)
}

fn parse_record(raw: &str, line: usize) -> Result<DynamicRecord, DyrError> {
    let fields = split_fields(raw, line)?;
    if fields.len() < 2 || fields[1].is_empty() {
        return Err(DyrError::MissingModelName { line });
    }
    let bus_id = fields[0]
        .parse::<u32>()
        .map_err(|_| DyrError::InvalidBusId {
            line,
            token: fields[0].clone(),
        })?;
    let model_name = fields[1].clone();
    let device_id = fields.get(2).cloned().filter(|value| !value.is_empty());
    let parameters = fields.into_iter().skip(3).collect();

    Ok(DynamicRecord {
        model_name,
        bus_id,
        device_id,
        parameters,
        line,
        raw_record: raw.to_owned(),
    })
}

fn split_fields(raw: &str, line: usize) -> Result<Vec<String>, DyrError> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut in_quote = false;
    let bytes = raw.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\'' {
            if in_quote && index + 1 < bytes.len() && bytes[index + 1] == b'\'' {
                index += 2;
                continue;
            }
            in_quote = !in_quote;
        } else if bytes[index] == b',' && !in_quote {
            fields.push(decode_field(&raw[start..index], line)?);
            if fields.len() > MAX_FIELDS_PER_RECORD {
                return Err(DyrError::TooManyFields {
                    line,
                    maximum: MAX_FIELDS_PER_RECORD,
                });
            }
            start = index + 1;
        }
        index += 1;
    }

    if in_quote {
        return Err(DyrError::UnterminatedQuote { line });
    }
    fields.push(decode_field(&raw[start..], line)?);
    if fields.len() > MAX_FIELDS_PER_RECORD {
        return Err(DyrError::TooManyFields {
            line,
            maximum: MAX_FIELDS_PER_RECORD,
        });
    }
    Ok(fields)
}

fn decode_field(raw: &str, line: usize) -> Result<String, DyrError> {
    let token = raw.trim();
    if token.len() > MAX_FIELD_BYTES {
        return Err(DyrError::FieldTooLong {
            line,
            maximum: MAX_FIELD_BYTES,
        });
    }
    if token.len() >= 2 && token.starts_with('\'') && token.ends_with('\'') {
        Ok(token[1..token.len() - 1].replace("''", "'"))
    } else {
        Ok(token.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_and_unknown_models_and_multiline_records() {
        let source = "! machine models\n\
            1, 'GENCLS', '1', 3.5, 0.2 /\n\
            1, 'USERMODEL', '2', 'alpha, beta', 0.4 /\n\
            2, 'EXDC1', '1', 0.1,\n\
            0.2, 5.0 /\n";
        let case = parse_dyr(source).expect("valid DYR");
        assert_eq!(case.records().len(), 3);
        assert_eq!(case.records()[0].model_name(), "GENCLS");
        assert_eq!(case.records()[0].bus_id(), 1);
        assert_eq!(case.records()[0].device_id(), Some("1"));
        assert_eq!(case.records()[0].parameters(), ["3.5", "0.2"]);
        assert_eq!(case.records()[1].model_name(), "USERMODEL");
        assert_eq!(case.records()[1].parameters()[0], "alpha, beta");
        assert_eq!(case.records()[2].parameters(), ["0.1", "0.2", "5.0"]);
        assert_eq!(case.to_dyr_string(), source);
    }

    #[test]
    fn quoted_slash_and_comment_markers_do_not_end_a_record() {
        let source = "1, 'USER/EXC', '1', 'not ! a comment', 1.0 / ! tail\n";
        let case = parse_dyr(source).expect("quoted delimiters");
        assert_eq!(case.records()[0].model_name(), "USER/EXC");
        assert_eq!(case.records()[0].parameters()[0], "not ! a comment");
        assert_eq!(case.to_dyr_string(), source);
    }

    #[test]
    fn preserves_empty_parameter_slots() {
        let case = parse_dyr("1, 'MODEL', '1', 1.0, , 3.0 /\n").unwrap();
        assert_eq!(case.records()[0].parameters(), ["1.0", "", "3.0"]);
    }

    #[test]
    fn reports_record_start_lines_after_blank_lines() {
        let case = parse_dyr("\n1, 'GENCLS', '1', 3.5 /\n\n2, 'EXDC1', '1', 1.0 /\n").unwrap();
        assert_eq!(case.records()[0].line(), 2);
        assert_eq!(case.records()[1].line(), 4);
    }

    #[test]
    fn reports_unresolved_bus_references() {
        let case = parse_dyr("1, 'GENCLS', '1', 3.5, 0.2 /\n2, 'EXDC1', '1', 1.0 /\n").unwrap();
        let ids = BTreeSet::from([1]);
        let unresolved = case.unresolved_buses(&ids);
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].bus_id, 2);
        assert_eq!(unresolved[0].model_name, "EXDC1");
    }

    #[test]
    fn rejects_truncated_records_and_bad_bus_ids() {
        assert!(matches!(
            parse_dyr("1, 'GENCLS', '1', 3.5"),
            Err(DyrError::UnterminatedRecord { .. })
        ));
        assert!(matches!(
            parse_dyr("bus, 'GENCLS', '1', 3.5 /"),
            Err(DyrError::InvalidBusId { .. })
        ));
    }

    #[test]
    fn rejects_unterminated_quotes_and_invalid_utf8() {
        assert!(matches!(
            parse_dyr("1, 'GENCLS, '1', 3.5 /"),
            Err(DyrError::UnterminatedQuote { .. })
        ));
        assert_eq!(parse_dyr_bytes(&[0xff]), Err(DyrError::InvalidUtf8));
    }

    #[test]
    fn enforces_input_size_limit_before_parsing() {
        let text = " ".repeat(MAX_DYR_BYTES + 1);
        assert!(matches!(
            parse_dyr(&text),
            Err(DyrError::InputTooLarge { .. })
        ));
    }
}
