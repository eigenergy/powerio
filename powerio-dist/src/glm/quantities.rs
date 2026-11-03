use num_complex::Complex64;
use powerio_core::{Diagnostic, Error};

use super::{GlmDialect, GlmProject, GlmProperty, GlmPropertyValue, TokenKind, units};

/// Native property storage kind. Quoted constants use a different native
/// conversion path from unquoted GLM expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlmQuantityKind {
    Real,
    Complex,
}

/// A constant in the property's declared native unit. Diagnostics must be
/// forwarded when projecting into a network, particularly for ignored suffixes.
#[derive(Clone, Debug)]
pub struct GlmQuantity {
    pub value: Complex64,
    pub diagnostics: Vec<Diagnostic>,
}

impl GlmProject {
    /// Read a finite constant using the native property kind and unit. The
    /// caller supplies the unit from the class schema, not from the input text.
    /// `None` denotes a property without units, not an unknown unit.
    ///
    /// Supports decimal/exponent numbers, rectangular `i`/`j`, polar `d`/`r`,
    /// and the bounded static electrical/geometry unit profile. Expressions,
    /// property references and custom unit definitions require further
    /// interpretation and fail here. Arras additionally accepts quoted,
    /// parenthesized complex literals. Source bytes and diagnostic spans remain.
    pub fn quantity(
        &self,
        property: &GlmProperty<'_>,
        kind: GlmQuantityKind,
        native_unit: Option<&str>,
    ) -> Result<GlmQuantity, Error> {
        let fail = |message| self.quantity_error(property.span, message);
        let GlmPropertyValue::Tokens(tokens) = &property.value else {
            return Err(fail("quantity cannot be an inline object"));
        };
        let quoted = tokens.len() == 1 && tokens[0].kind() == TokenKind::Quoted;
        let text = if let [token] = *tokens
            && token.kind() == TokenKind::Quoted
        {
            token
                .text()
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .ok_or_else(|| fail("quantity strings require double quotes"))?
                .to_owned()
        } else {
            let mut text = String::new();
            for token in *tokens {
                if token.kind() != TokenKind::Word {
                    return Err(fail("quantity contains nonliteral syntax"));
                }
                if text.len() + token.text().len() + 1 > 1 << 20 {
                    return Err(fail("quantity exceeds 1 MiB"));
                }
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(token.text());
            }
            text
        };
        let (value, ignored) =
            parse(&text, kind, native_unit, self.dialect(), quoted).map_err(fail)?;
        let diagnostics = if let Some(suffix) = ignored {
            vec![self.ignored_quantity_unit(property.span, suffix)?]
        } else {
            Vec::new()
        };
        Ok(GlmQuantity { value, diagnostics })
    }
}

fn parse<'a>(
    text: &'a str,
    kind: GlmQuantityKind,
    unit: Option<&str>,
    dialect: GlmDialect,
    quoted: bool,
) -> Result<(Complex64, Option<&'a str>), &'static str> {
    if quoted && text.is_empty() && kind == GlmQuantityKind::Complex {
        return Ok((Complex64::new(0.0, 0.0), None));
    }
    let text = text.trim();
    let (text, parenthesized) = if text.starts_with('(') {
        if !quoted || kind != GlmQuantityKind::Complex || dialect != GlmDialect::Arras4_3 {
            return Err("parenthesized quantity requires a quoted Arras complex literal");
        }
        (
            text.strip_prefix('(')
                .and_then(|s| s.strip_suffix(')'))
                .ok_or("parenthesized complex literal must have no trailing suffix")?,
            true,
        )
    } else {
        (text, false)
    };
    let (first, rest) = number(text)?;
    if kind == GlmQuantityKind::Real {
        let suffix = rest.trim();
        let value = if suffix.is_empty() {
            first
        } else if let Some(unit) = unit {
            units::convert(first, suffix, unit)?
        } else if !quoted {
            units::convert(1.0, suffix, suffix)?;
            return Ok((Complex64::new(first, 0.0), Some(suffix)));
        } else {
            return Err("quoted real property does not accept units");
        };
        return Ok((Complex64::new(value, 0.0), None));
    }
    let rest = rest.trim_start();
    // Native sscanf stops after the first number if the suffix does not start
    // another number. It consequently never reaches complex unit conversion.
    let starts_number = rest
        .as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.'));
    if !starts_number {
        if parenthesized && !rest.is_empty() {
            return Err("invalid parenthesized complex literal");
        }
        let suffix = rest.trim();
        let value = Complex64::new(first, 0.0);
        return if quoted {
            Ok((value, (!suffix.is_empty()).then_some(suffix)))
        } else {
            convert_complex(value, suffix, unit, parenthesized)
        };
    }
    let (second, rest) = number(rest)?;
    let notation = rest
        .as_bytes()
        .first()
        .copied()
        .ok_or("complex value lacks i, j, d or r notation")?;
    let value = match notation {
        b'i' | b'j' => Complex64::new(first, second),
        b'd' => Complex64::from_polar(first, second.to_radians()),
        b'r' => Complex64::from_polar(first, second),
        _ => return Err("complex value lacks i, j, d or r notation"),
    };
    let suffix = rest[1..].trim();
    if parenthesized && !suffix.is_empty() {
        return Err("parenthesized complex literal cannot contain a unit suffix");
    }
    convert_complex(value, suffix, unit, quoted || parenthesized)
}

fn convert_complex<'a>(
    value: Complex64,
    suffix: &'a str,
    unit: Option<&str>,
    string_conversion: bool,
) -> Result<(Complex64, Option<&'a str>), &'static str> {
    let (value, ignored) = if suffix.is_empty() {
        (value, None)
    } else if let Some(unit) = unit {
        let converted = if string_conversion {
            value * units::convert(1.0, suffix, unit)?
        } else {
            Complex64::new(
                units::convert(value.re, suffix, unit)?,
                units::convert(value.im, suffix, unit)?,
            )
        };
        (converted, None)
    } else {
        if !string_conversion {
            units::convert(1.0, suffix, suffix)?;
        }
        (value, Some(suffix))
    };
    if !value.re.is_finite() || !value.im.is_finite() {
        return Err("complex conversion produced a nonfinite value");
    }
    Ok((value, ignored))
}

// A bounded decimal scanner avoids confusing exponent signs with the second
// component, while refusing sscanf's partial malformed-number acceptance.
fn number(text: &str) -> Result<(f64, &str), &'static str> {
    let bytes = text.as_bytes();
    let mut end = usize::from(bytes.first().is_some_and(|b| matches!(b, b'+' | b'-')));
    let mut digits = 0;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
        digits += 1;
    }
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return Err("expected a finite decimal quantity");
    }
    if bytes.get(end).is_some_and(|b| matches!(b, b'e' | b'E')) {
        end += 1;
        if bytes.get(end).is_some_and(|b| matches!(b, b'+' | b'-')) {
            end += 1;
        }
        let exponent_start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == exponent_start {
            return Err("quantity exponent has no digits");
        }
    }
    let value = text[..end]
        .parse::<f64>()
        .map_err(|_| "invalid quantity number")?;
    if !value.is_finite() {
        return Err("quantity must be finite");
    }
    Ok((value, &text[end..]))
}
