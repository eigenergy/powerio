//! Shielded cable equations shared by the OpenDSS cable line constants
//! (CNData and TSData) and the GridLAB-D underground line reader: the
//! concentric neutral ring, the tape shield's mean radius, and the shunt
//! capacitance between a phase core and its screen.
//!
//! Every function works in the caller's own length unit; only ratios of
//! lengths enter the logarithms.

/// The radius of the circle through a concentric neutral's strand centres:
/// half the cable's diameter over the neutral less one strand diameter.
#[must_use]
pub(crate) fn calc_concentric_neutral_radius(outer_diameter: f64, strand_diameter: f64) -> f64 {
    0.5 * (outer_diameter - strand_diameter)
}

/// The mean radius of a tape shield, to the centre of the tape: half the
/// diameter over the shield less one tape thickness (Kersting). OpenDSS
/// takes it as the shield's GMR, GridLAB-D as the capacitance radius.
#[must_use]
pub(crate) fn calc_tape_shield_radius(diameter_over_shield: f64, thickness: f64) -> f64 {
    0.5 * (diameter_over_shield - thickness)
}

/// The equivalent GMR of `strands` round strands of GMR `strand_gmr` on a
/// circle of radius `ring_radius`: `(GMR_s k R^(k-1))^(1/k)`.
#[must_use]
pub(crate) fn calc_concentric_neutral_gmr(strand_gmr: f64, strands: f64, ring_radius: f64) -> f64 {
    (strand_gmr * strands * ring_radius.powf(strands - 1.0)).powf(1.0 / strands)
}

/// The equivalent distance between a concentric neutral of `strands`
/// strands on a circle of radius `ring_radius` and a conductor `distance`
/// from the cable's centre: `(D^k - R^k)^(1/k)`.
#[must_use]
pub(crate) fn calc_concentric_neutral_distance(
    distance: f64,
    strands: f64,
    ring_radius: f64,
) -> f64 {
    (distance.powf(strands) - ring_radius.powf(strands)).powf(1.0 / strands)
}

/// The shunt capacitance per unit length between a phase core of radius
/// `inner_radius` and its grounded screen at `outer_radius`:
///
/// ```text
/// C = 2π ε / (ln(R / r) − ln(k r_s / R) / k)
/// ```
///
/// `two_pi_permittivity` is `2π ε` for the insulation, in the caller's
/// engine constants and unit (OpenDSS rounds 2π to 6.283185307 and states
/// ε₀ per meter; GridLAB-D states 2π ε₀ per mile). The strand term applies
/// only to a concentric neutral of `k` strands of radius `r_s` stated
/// through `strands` (Kersting); a solid screen, or the coaxial form OpenDSS
/// uses for both cable kinds, passes `None` and reads `C = 2π ε / ln(R / r)`.
#[must_use]
pub(crate) fn calc_cable_capacitance(
    two_pi_permittivity: f64,
    outer_radius: f64,
    inner_radius: f64,
    strands: Option<(f64, f64)>,
) -> f64 {
    let strand_term = strands.map_or(0.0, |(k, strand_radius)| {
        (k * strand_radius / outer_radius).ln() / k
    });
    two_pi_permittivity / ((outer_radius / inner_radius).ln() - strand_term)
}
