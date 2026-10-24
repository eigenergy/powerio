"""Selection values for SINCAL's balanced reader."""
from dataclasses import dataclass
from typing import Optional


def _validate_selection(value):
    if value.variant is not None and (isinstance(value.variant, bool) or not isinstance(value.variant, int)):
        raise TypeError("variant must be an integer or None")
    if value.snapshot_hours is not None and (isinstance(value.snapshot_hours, bool) or not isinstance(value.snapshot_hours, (int, float))):
        raise TypeError("snapshot_hours must be a number or None")
    if value.acquired_tables is not None and not isinstance(value.acquired_tables, str):
        raise TypeError("acquired_tables must be a relative companion name or None")


@dataclass(frozen=True)
class SincalBalancedReadOptions:
    """Native variant, daily snapshot and optional verified Access companion.

    Use with ``parse(..., format="sincal-balanced", sincal_balanced=...)``.
    Parsing never invokes an acquisition tool. The primary source remains the MDB.
    """
    variant: Optional[int] = None
    snapshot_hours: Optional[float] = None
    acquired_tables: Optional[str] = None

    def __post_init__(self):
        _validate_selection(self)
