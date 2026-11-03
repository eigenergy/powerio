//! Native SINCAL acquisition and explicitly selected electrical profiles.
mod balanced;
#[allow(dead_code)]
mod multiconductor;
#[doc(hidden)]
pub mod storage;
#[cfg(test)]
mod testkit;
pub use balanced::source::parse as parse_balanced;
pub use balanced::source::{
    SincalBalancedReadOptions, parse_with_options as parse_sincal_balanced_with_options,
};
#[doc(hidden)]
pub use balanced::{
    ExperimentalBalancedOutput, read_balanced_snapshot as __read_sincal_balanced_snapshot,
    read_balanced_snapshot_at as __read_sincal_balanced_snapshot_at,
    write_experimental_balanced as __write_sincal_balanced_experimental,
};
pub use multiconductor::public::{SincalReadOptions, parse_sincal};

/// Internal SINCAL validation entry point; callers must explicitly choose the
/// conductor-resolved profile. Unsupported inputs reject the whole network.
///
/// # Errors
/// Unverified schema, unsupported electrical modes or inconsistent inputs.
#[doc(hidden)]
pub fn __read_sincal_multiconductor_snapshot(
    snapshot: storage::DatabaseSnapshot,
) -> powerio_dist::Result<powerio_dist::MulticonductorNetwork> {
    multiconductor::read_snapshot(snapshot)
}

/// Internal component audit. A successful report is not a parsed network.
///
/// # Errors
/// Unsupported schema or inability to establish the common mapping context.
#[doc(hidden)]
pub fn __audit_sincal_multiconductor_snapshot(
    snapshot: storage::DatabaseSnapshot,
) -> powerio_dist::Result<String> {
    multiconductor::audit_snapshot(snapshot)
}

/// Internal SINCAL snapshot selection in hours for verified daily load profiles.
///
/// # Errors
/// Unsupported profiles or components, invalid time, or inconsistent inputs.
#[doc(hidden)]
pub fn __read_sincal_multiconductor_at(
    snapshot: storage::DatabaseSnapshot,
    hours: f64,
) -> powerio_dist::Result<powerio_dist::MulticonductorNetwork> {
    multiconductor::read_snapshot_at(snapshot, hours)
}

/// Internal component audit at an explicit daily-profile time; not a partial network.
///
/// # Errors
/// Unsupported common context or invalid snapshot time.
#[doc(hidden)]
pub fn __audit_sincal_multiconductor_at(
    snapshot: storage::DatabaseSnapshot,
    hours: f64,
) -> powerio_dist::Result<String> {
    multiconductor::audit_snapshot_at(snapshot, hours)
}

#[doc(hidden)]
pub use multiconductor::{
    ExperimentalMulticonductorOptions, ExperimentalMulticonductorOutput,
    write_experimental_multiconductor as __write_sincal_multiconductor_experimental,
};
