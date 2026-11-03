//! Family-local error adaptation over shared native transport.

use std::borrow::Cow;

pub(super) use crate::formats::sincal::storage::MAX_BYTES;

pub(super) fn database_bytes(bytes: &[u8]) -> powerio_dist::Result<Cow<'_, [u8]>> {
    crate::formats::sincal::storage::database_bytes(bytes).map_err(super::format_error)
}
