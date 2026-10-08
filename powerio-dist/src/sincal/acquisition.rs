//! Family-local error adaptation over shared native transport.

use std::borrow::Cow;

pub(super) use powerio_sincal::MAX_BYTES;

pub(super) fn database_bytes(bytes: &[u8]) -> crate::Result<Cow<'_, [u8]>> {
    powerio_sincal::database_bytes(bytes).map_err(super::format_error)
}
