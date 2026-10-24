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
                // A record keyed by a device name references no bus here.
                let bus_id = record.bus_id?;
                (!bus_ids.contains(&bus_id)).then(|| UnresolvedBusReference {
                    line: record.line,
                    model_name: record.model_name.clone(),
                    bus_id,
                })
            })
            .collect()
    }

    /// Check generic DYR bus references against a parsed balanced network.
    ///
    /// This deliberately does not validate `device_id`: there is no
    /// model-independent rule that the second DYR key names a generator.
    #[must_use]
    pub fn unresolved_buses_in(
        &self,
        network: &crate::BalancedNetwork,
    ) -> Vec<UnresolvedBusReference> {
        let bus_ids: BTreeSet<u32> = network
            .buses()
            .iter()
            .filter_map(|bus| u32::try_from(bus.id.0).ok())
            .collect();
        self.unresolved_buses(&bus_ids)
    }
}

/// A steady-state case explicitly paired with its transient-stability data.
///
/// This is a study-level association, not a mutation of [`crate::BalancedNetwork`].
/// The current linking guarantee validates generic bus references only; it does
/// not resolve model-specific device keys or claim that the named device exists.
#[derive(Clone, Debug)]
pub struct DynamicStudy {
    network: crate::BalancedNetwork,
    dynamics: DynamicCase,
}

impl DynamicStudy {
    /// Pair a parsed static network with a parsed DYR case.
    #[must_use]
    pub fn new(network: crate::BalancedNetwork, dynamics: DynamicCase) -> Self {
        Self { network, dynamics }
    }

    /// The steady-state network, unchanged by the pairing.
    #[must_use]
    pub fn network(&self) -> &crate::BalancedNetwork {
        &self.network
    }

    /// The loss-preserving dynamic-model records.
    #[must_use]
    pub fn dynamics(&self) -> &DynamicCase {
        &self.dynamics
    }

    /// DYR records whose bus reference is absent from the static case.
    #[must_use]
    pub fn unresolved_buses(&self) -> Vec<UnresolvedBusReference> {
        self.dynamics.unresolved_buses_in(&self.network)
    }

    /// Consume the study and recover the separately represented components.
    #[must_use]
    pub fn into_parts(self) -> (crate::BalancedNetwork, DynamicCase) {
        (self.network, self.dynamics)
    }
}

/// One DYR model record. Parameters remain ordered and lexically decoded;
/// model-specific interpretation belongs in a separate, schema-backed layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicRecord {
    model_name: String,
    key: String,
    bus_id: Option<u32>,
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

    /// The first DYR field, without surrounding quotes. Most models key a
    /// record by an unquoted bus number; dc line and similar models name
    /// their device here in quotes instead.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Bus number in an unquoted first field, or `None` for a record keyed
    /// by a quoted device name such as a dc line. Load models keyed by area,
    /// zone or owner state that number here too; it is not a bus.
    #[must_use]
    pub fn bus_id(&self) -> Option<u32> {
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
#[non_exhaustive]
pub struct UnresolvedBusReference {
    pub line: usize,
    pub model_name: String,
    pub bus_id: u32,
}

/// Errors detected while reading a DYR source.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
/// starts a line-comment outside quotes. Fields are separated by commas or ASCII whitespace; single-
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
    let mut record_started = false;
    let bytes = uncommented.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                line += 1;
                if !record_started {
                    start_line = line;
                }
                index += 1;
            }
            b'/' if !in_quote => {
                if record_started {
                    if records.len() >= MAX_RECORDS {
                        return Err(DyrError::TooManyRecords {
                            maximum: MAX_RECORDS,
                        });
                    }
                    records.push(parse_record(uncommented[start..index].trim(), start_line)?);
                }
                start = index + 1;
                start_line = line;
                record_started = false;
                index += 1;
            }
            b'\'' => {
                if !record_started {
                    record_started = true;
                    start_line = line;
                }
                if in_quote && index + 1 < bytes.len() && bytes[index + 1] == b'\'' {
                    index += 2;
                } else {
                    in_quote = !in_quote;
                    index += 1;
                }
            }
            byte => {
                if !record_started && !byte.is_ascii_whitespace() {
                    record_started = true;
                    start_line = line;
                }
                index += 1;
            }
        }
    }

    if record_started {
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
    // An unquoted key is a bus number; a quoted key names a device, as dc
    // line models do (`'DC1' 'CDC4T' ... /`).
    let bus_id = if raw.trim_start().starts_with('\'') {
        None
    } else {
        Some(
            fields[0]
                .parse::<u32>()
                .map_err(|_| DyrError::InvalidBusId {
                    line,
                    token: fields[0].clone(),
                })?,
        )
    };
    let key = fields[0].clone();
    let model_name = fields[1].clone();
    let device_id = fields.get(2).cloned().filter(|value| !value.is_empty());
    let parameters = fields.into_iter().skip(3).collect();

    Ok(DynamicRecord {
        model_name,
        key,
        bus_id,
        device_id,
        parameters,
        line,
        raw_record: raw.to_owned(),
    })
}

fn split_fields(raw: &str, line: usize) -> Result<Vec<String>, DyrError> {
    let mut fields = Vec::new();
    let mut token_start = None;
    let mut in_quote = false;
    let mut pending_comma = false;
    let bytes = raw.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' => {
                if token_start.is_none() {
                    token_start = Some(index);
                    pending_comma = false;
                }
                if in_quote && index + 1 < bytes.len() && bytes[index + 1] == b'\'' {
                    index += 2;
                    continue;
                }
                in_quote = !in_quote;
                index += 1;
            }
            b',' if !in_quote => {
                if let Some(start) = token_start.take() {
                    push_decoded_field(&mut fields, &raw[start..index], line)?;
                } else if pending_comma || fields.is_empty() {
                    push_decoded_field(&mut fields, "", line)?;
                }
                pending_comma = true;
                index += 1;
            }
            byte if byte.is_ascii_whitespace() && !in_quote => {
                if let Some(start) = token_start.take() {
                    push_decoded_field(&mut fields, &raw[start..index], line)?;
                    pending_comma = false;
                }
                index += 1;
            }
            _ => {
                if token_start.is_none() {
                    token_start = Some(index);
                    pending_comma = false;
                }
                index += 1;
            }
        }
    }

    if in_quote {
        return Err(DyrError::UnterminatedQuote { line });
    }
    if let Some(start) = token_start {
        push_decoded_field(&mut fields, &raw[start..], line)?;
    } else if pending_comma {
        push_decoded_field(&mut fields, "", line)?;
    }
    Ok(fields)
}

fn push_decoded_field(fields: &mut Vec<String>, raw: &str, line: usize) -> Result<(), DyrError> {
    fields.push(decode_field(raw, line)?);
    if fields.len() > MAX_FIELDS_PER_RECORD {
        return Err(DyrError::TooManyFields {
            line,
            maximum: MAX_FIELDS_PER_RECORD,
        });
    }
    Ok(())
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
        assert_eq!(case.records()[0].bus_id(), Some(1));
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
    fn quoted_device_keys_are_names_not_buses() {
        // PSS/E dc line models key a record by its quoted line name, as in
        // the Model Library's `'DC Line Name', 'USRDCL', 'CHVDC2U1' ...` record.
        let source = "'DC1' 'CDC4T' 0.1 0.2 /\n\
            '1', 'USRDCL', 'CHVDC2U1', 18, 0.5 /\n\
            3, 'GENCLS', '1', 3.5 /\n";
        let case = parse_dyr(source).expect("named dc line records parse");
        let records = case.records();
        assert_eq!((records[0].key(), records[0].bus_id()), ("DC1", None));
        assert_eq!(records[0].model_name(), "CDC4T");
        assert_eq!((records[1].key(), records[1].bus_id()), ("1", None));
        assert_eq!((records[2].key(), records[2].bus_id()), ("3", Some(3)));
        // A dc line named '1' is not bus 1, and named records reference no bus.
        let unresolved = case.unresolved_buses(&BTreeSet::from([1]));
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].bus_id, 3);
        assert_eq!(case.to_dyr_string(), source);
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
    #[test]
    fn accepts_comma_whitespace_mixed_tab_and_multiline_dyr_records() {
        let cases = [
            "1, 'GENCLS', '1', 13.0000, 0.000000 /",
            "1 'GENCLS' '1' 13.0000 0.000000 /",
            "1, 'GENCLS', '1', 13.0000 0.000000 /",
            "1\t'GENCLS'\t'1'\t13.0000\t0.000000 /",
            "1, 'GENCLS',\n  '1', 13.0000\n 0.000000 /",
        ];
        for source in cases {
            let case = parse_dyr(source).unwrap_or_else(|error| panic!("{source:?}: {error}"));
            assert_eq!(case.records().len(), 1, "{source:?}");
            let record = &case.records()[0];
            assert_eq!(record.bus_id(), Some(1), "{source:?}");
            assert_eq!(record.model_name(), "GENCLS", "{source:?}");
            assert_eq!(record.device_id(), Some("1"), "{source:?}");
            assert_eq!(record.parameters(), ["13.0000", "0.000000"], "{source:?}");
            assert_eq!(case.to_dyr_string(), source);
        }
    }

    #[test]
    fn quoted_whitespace_commas_and_explicit_empty_fields_are_preserved() {
        let source = "1, 'USERMODEL', '1', 'alpha, beta', 1.0, , 3.0 /";
        let case = parse_dyr(source).unwrap();
        assert_eq!(case.records()[0].model_name(), "USERMODEL");
        assert_eq!(
            case.records()[0].parameters(),
            ["alpha, beta", "1.0", "", "3.0"]
        );
        assert_eq!(case.to_dyr_string(), source);

        let quoted = "1, 'USERMODEL', '1', 'O''Brien, alpha', 1.0 /";
        let case = parse_dyr(quoted).unwrap();
        assert_eq!(case.records()[0].parameters(), ["O'Brien, alpha", "1.0"]);
        assert_eq!(case.to_dyr_string(), quoted);
    }

    #[test]
    fn long_blank_and_comment_prefixes_are_scanned_once() {
        let source = format!(
            "{}{}1 'GENCLS' '1' 13.0 0.0 /\n",
            "\n".repeat(100_000),
            "! comment\n".repeat(100_000)
        );
        let case = parse_dyr(&source).expect("long blank/comment prefix");
        assert_eq!(case.records().len(), 1);
        assert_eq!(case.records()[0].line(), 200_001);
    }

    #[test]
    fn paired_dynamic_study_checks_bus_ids_but_keeps_device_keys_opaque() {
        let network = crate::BalancedNetwork::in_memory(
            "static",
            100.0,
            vec![crate::Bus::new(crate::BusId(1), crate::BusType::Ref, 230.0)],
            Vec::new(),
        );
        let dynamics =
            parse_dyr("1, 'GENCLS', '1', 13.0, 0.0 /\n2, 'USERMODEL', 'CUSTOM-A', 1.0 /\n")
                .expect("DYR parses");
        let study = DynamicStudy::new(network, dynamics);
        let unresolved = study.unresolved_buses();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].bus_id, 2);
        assert_eq!(unresolved[0].model_name, "USERMODEL");
        assert_eq!(study.dynamics().records()[1].device_id(), Some("CUSTOM-A"));
    }
}
