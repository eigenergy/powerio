//! Regression crate for the facade's completeness: this package depends on
//! `powerio` alone, never `powerio-core`. If a future edit stops
//! re-exporting a type the crate level README example names, this fails to
//! compile with an unresolved name here rather than a downstream consumer
//! finding out first.

#[cfg(test)]
mod tests {
    #[test]
    fn the_readme_example_resolves_through_the_facade_alone() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/case9.m");
        let module = powerio::parse(powerio::Source::open(&path).unwrap()).unwrap();
        let module: powerio::PioModule<powerio::BalancedNetwork> =
            powerio::try_into_typed(module).unwrap();
        assert!(!module.value().buses().is_empty());
    }
}
