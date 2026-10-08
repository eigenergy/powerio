// Electrical/geometry unit profile from the pinned engines' unitfile.txt.
// Values below are unit definitions, not a replacement simulator unit manager.
// Unknown names and unsupported derivations fail; process files are never read.

#[derive(Clone, Copy)]
struct Unit {
    // kg, m, s, A, K. Angles follow the native dimensionless unit convention.
    dimensions: [i16; 5],
    scale: f64,
    bias: f64,
}

impl Unit {
    const fn new(dimensions: [i16; 5], scale: f64) -> Self {
        Self {
            dimensions,
            scale,
            bias: 0.0,
        }
    }
}

pub(super) fn convert(value: f64, from: &str, to: &str) -> Result<f64, &'static str> {
    let from = parse(from)?;
    let to = parse(to)?;
    if from.dimensions != to.dimensions {
        return Err("quantity units have incompatible dimensions");
    }
    let result = (value - from.bias) * (from.scale / to.scale) + to.bias;
    if !result.is_finite() {
        return Err("unit conversion produced a nonfinite value");
    }
    Ok(result)
}

fn parse(text: &str) -> Result<Unit, &'static str> {
    if text.is_empty() || text.len() > 256 {
        return Err("unit spelling is empty or exceeds 256 bytes");
    }
    if let Some(unit) = named(text) {
        return Ok(unit);
    }
    let mut result = Unit::new([0; 5], 1.0);
    let mut start = 0;
    let mut divide = false;
    let mut factors = 0;
    for end in text
        .match_indices(['*', '/'])
        .map(|(i, _)| i)
        .chain([text.len()])
    {
        factors += 1;
        if factors > 32 {
            return Err("unit expression exceeds 32 factors");
        }
        let term = &text[start..end];
        let (name, exponent) = if let Some((name, exponent)) = term.split_once('^') {
            let exponent = exponent
                .parse::<i16>()
                .map_err(|_| "invalid unit exponent")?;
            if !(2..=16).contains(&exponent) {
                return Err("unit exponent must be an integer from 2 through 16");
            }
            (name, exponent)
        } else {
            (term, 1)
        };
        let unit = named(name).ok_or("unknown unit in the static electrical profile")?;
        if unit.bias != 0.0 {
            return Err("offset temperature units cannot be composed");
        }
        let exponent = if divide { -exponent } else { exponent };
        for (dimension, factor) in result.dimensions.iter_mut().zip(unit.dimensions) {
            *dimension += factor * exponent;
        }
        result.scale *= unit.scale.powi(i32::from(exponent));
        if end < text.len() {
            divide = text.as_bytes()[end] == b'/';
            start = end + 1;
        }
    }
    if !result.scale.is_finite() || result.scale <= 0.0 {
        return Err("unit scale overflows or underflows");
    }
    Ok(result)
}

fn named(name: &str) -> Option<Unit> {
    if let Some(unit) = base(name) {
        return Some(unit);
    }
    // Exact names take precedence: m is metre, h is hour, and F is farad.
    // Prefixes apply once, as in the native unit manager.
    for (prefix, exponent) in [
        ("Y", 24),
        ("Z", 21),
        ("E", 18),
        ("P", 15),
        ("T", 12),
        ("G", 9),
        ("M", 6),
        ("k", 3),
        ("h", 2),
        ("d", -1),
        ("da", 1),
        ("c", -2),
        ("m", -3),
        ("u", -6),
        ("n", -9),
        ("p", -12),
        ("f", -15),
        ("a", -18),
        ("z", -21),
        ("y", -24),
    ] {
        if let Some(suffix) = name.strip_prefix(prefix) {
            let mut unit = base(suffix)?;
            unit.scale *= 10_f64.powi(exponent);
            return Some(unit);
        }
    }
    None
}

fn base(name: &str) -> Option<Unit> {
    let (dimensions, scale) = match name {
        "1" | "unit" | "ratio" | "pu" => ([0; 5], 1.0),
        "%" => ([0; 5], 0.01),
        "m" => ([0, 1, 0, 0, 0], 1.0),
        "in" => ([0, 1, 0, 0, 0], 0.0254),
        "ft" => ([0, 1, 0, 0, 0], 0.3048),
        "yd" => ([0, 1, 0, 0, 0], 0.9144),
        "mile" => ([0, 1, 0, 0, 0], 1609.344),
        "sf" => ([0, 2, 0, 0, 0], 0.3048 * 0.3048),
        "sy" => ([0, 2, 0, 0, 0], 0.9144 * 0.9144),
        "g" => ([1, 0, 0, 0, 0], 0.001),
        "s" => ([0, 0, 1, 0, 0], 1.0),
        "min" => ([0, 0, 1, 0, 0], 60.0),
        "h" => ([0, 0, 1, 0, 0], 3600.0),
        "day" => ([0, 0, 1, 0, 0], 86400.0),
        "Hz" => ([0, 0, -1, 0, 0], 1.0),
        "A" => ([0, 0, 0, 1, 0], 1.0),
        "V" => ([1, 2, -3, -1, 0], 1.0),
        "W" | "VA" | "VAr" => ([1, 2, -3, 0, 0], 1.0),
        "Wh" | "VAh" => ([1, 2, -2, 0, 0], 3600.0),
        "J" => ([1, 2, -2, 0, 0], 1.0),
        "Ohm" | "ohm" => ([1, 2, -3, -2, 0], 1.0),
        "S" => ([-1, -2, 3, 2, 0], 1.0),
        "F" => ([-1, -2, 4, 2, 0], 1.0),
        "H" => ([1, 2, -2, -2, 0], 1.0),
        "C" => ([0, 0, 1, 1, 0], 1.0),
        "K" => ([0, 0, 0, 0, 1], 1.0),
        "R" => ([0, 0, 0, 0, 1], 0.555_555_56),
        // Preserve the pinned native definitions, including their rounding.
        "rad" => ([0; 5], 0.159_155),
        "deg" => ([0; 5], 0.002_777_777_8),
        "degC" => {
            return Some(Unit {
                dimensions: [0, 0, 0, 0, 1],
                scale: 1.0,
                bias: -273.14,
            });
        }
        "degF" => {
            return Some(Unit {
                dimensions: [0, 0, 0, 0, 1],
                scale: 0.555_555_56,
                bias: -459.65,
            });
        }
        _ => return None,
    };
    Some(Unit::new(dimensions, scale))
}
