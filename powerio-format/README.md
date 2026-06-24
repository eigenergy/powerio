# powerio-format

Canonical format alias and JSON shape detection shared by the powerio crates.

This crate has no model or parser dependencies. It only classifies names and
top level JSON markers so CLI, bindings, and parsers do not grow divergent
sniffing rules.
