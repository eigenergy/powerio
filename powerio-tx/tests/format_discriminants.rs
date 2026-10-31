//! Numeric casts of the published fieldless enums remain compatible when
//! formats are added. The payload-bearing TargetFormat is covered by the
//! cargo-semver-checks gate; new variants belong at the end of all three.

use powerio_tx::SourceFormat;
use powerio_tx::format::routing::TransmissionFormat;

#[test]
fn published_source_format_discriminants_are_stable() {
    use SourceFormat as F;
    let published = [
        F::Matpower,
        F::PowerModelsJson,
        F::EgretJson,
        F::Psse,
        F::PsseRawx,
        F::PowerWorld,
        F::PandapowerJson,
        F::Pslf,
        F::PowerWorldBinary,
        F::InMemory,
        F::Normalized,
        F::Gridfm,
        F::PypsaCsv,
        F::Goc3Json,
        F::SurgeJson,
        F::DeepMindOpfDataJson,
        F::Xiidm,
        F::Jiidm,
        F::Cgmes,
        F::Ucte,
        F::IeeeCdf,
        F::Sincal,
    ];
    for (value, format) in published.into_iter().enumerate() {
        assert_eq!(format as usize, value, "{format:?}");
    }
}

#[test]
fn published_transmission_format_discriminants_are_stable() {
    use TransmissionFormat as F;
    let published = [
        F::Matpower,
        F::PowerModelsJson,
        F::EgretJson,
        F::Psse,
        F::Psse34,
        F::Psse35,
        F::PsseRawx,
        F::PowerWorld,
        F::PandapowerJson,
        F::PypsaCsv,
        F::Pslf,
        F::Pwb,
        F::Gridfm,
        F::Goc3Json,
        F::SurgeJson,
        F::DeepMindOpfDataJson,
        F::Xiidm,
        F::Jiidm,
        F::Cgmes,
        F::Ucte,
        F::IeeeCdf,
        F::SincalBalanced,
    ];
    for (value, format) in published.into_iter().enumerate() {
        assert_eq!(format as usize, value, "{format:?}");
    }
}
