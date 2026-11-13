//! Array setters leave the last winding active in native OpenDSS.
// The asserted values are read verbatim from the script, so exact equality
// is the property under test.
#![allow(clippy::float_cmp)]
use powerio_core::Source;
use powerio_dist::{DistTransformer, DistWindingConn};

fn transformer(script: &str) -> DistTransformer {
    let source = Source::from_memory("case.dss", script.as_bytes()).unwrap();
    powerio_dist::parse(source).unwrap().value().transformers()[0].clone()
}

#[test]
fn arrays_select_last_winding_before_scalar_properties_and_across_edits() {
    for array in [
        "buses=[a b]",
        "conns=[delta wye]",
        "kvs=[11 .4]",
        "kvas=[500 500]",
        "taps=[1 1]",
        "%rs=[.2 .2]",
        // A short array preserves omitted values, but still selects winding 2.
        "buses=[a]",
        "conns=[delta]",
        "kvs=[11]",
        "kvas=[500]",
        "taps=[1]",
        "%rs=[.2]",
    ] {
        for edit in [" ", "\nedit transformer.t "] {
            let script = format!(
                "new circuit.c\nnew transformer.t windings=2 buses=[a b] wdg=1 {array}{edit}rneut=0 %r=.52 tap=1.03 kv=.42\n"
            );
            let t = transformer(&script);
            let [first, last] = t.windings.as_slice() else {
                panic!("{array}")
            };
            assert_eq!(first.r_neutral, None, "{array}");
            assert_eq!(first.r_pct, 0.2, "{array}");
            assert_eq!(last.r_neutral, Some(0.0), "{array}");
            assert_eq!(last.r_pct, 0.52, "{array}");
            assert_eq!(last.tap, 1.03, "{array}");
            assert_eq!(last.v_ref, 420.0, "{array}");
        }
    }
}

#[test]
fn explicit_selector_overrides_arrays_and_reactance_arrays_do_not_select() {
    for suffix in ["wdg=1", "wdg=1 xscarray=[7]"] {
        let t = transformer(&format!(
            "new circuit.c\nnew transformer.t windings=2 buses=[a b] kvs=[11 .4] {suffix} rneut=0\n"
        ));
        assert_eq!(t.windings[0].r_neutral, Some(0.0));
        assert_eq!(t.windings[1].r_neutral, None);
    }
}

#[test]
fn xfmrcode_arrays_leave_last_winding_selected() {
    for array in [
        "conns=[delta wye]",
        "kvs=[11 .4]",
        "kvas=[500 500]",
        "taps=[1 1]",
        "%rs=[.2 .2]",
    ] {
        for edit in [" ", "\nedit xfmrcode.c "] {
            let t = transformer(&format!(
                "new circuit.c\nnew xfmrcode.c windings=2 wdg=1 {array}{edit}rneut=0 %r=.52\nnew transformer.t xfmrcode=c buses=[a b]\n"
            ));
            assert_eq!(t.windings[0].r_neutral, Some(-1.0), "{array}");
            assert_eq!(t.windings[0].r_pct, 0.2, "{array}");
            assert_eq!(t.windings[1].r_neutral, Some(0.0), "{array}");
            assert_eq!(t.windings[1].r_pct, 0.52, "{array}");
        }
    }
}

#[test]
fn delta_wye_array_declaration_grounds_only_the_wye_neutral() {
    let t = transformer(
        "new circuit.c\nnew transformer.t windings=2 buses=[a b.1.2.3.4] conns=[delta wye] kvs=[11 .4] kvas=[500 500] %r=(.52 .52) xhl=7.5 rneut=0\n",
    );
    assert_eq!(t.windings[0].conn, DistWindingConn::Delta);
    assert_eq!(t.windings[0].r_neutral, None);
    assert_eq!(t.windings[0].r_pct, 0.2);
    assert_eq!(t.windings[1].conn, DistWindingConn::Wye);
    assert_eq!(t.windings[1].r_neutral, Some(0.0));
    assert_eq!(t.windings[1].r_pct, 0.52);
}

#[test]
fn arrays_follow_the_current_winding_count_after_an_edit() {
    for class in ["transformer", "xfmrcode"] {
        let mut script = format!(
            "new circuit.c\nnew {class}.t windings=2 kvs=[11 .4]\nedit {class}.t windings=3 kvs=[11 .4 .2] rneut=0\n"
        );
        if class == "xfmrcode" {
            script.push_str("new transformer.t xfmrcode=t buses=[a b c]\n");
        } else {
            script.push_str("edit transformer.t buses=[a b c]\n");
        }
        let t = transformer(&script);
        let neutrals: Vec<_> = t
            .windings
            .iter()
            .map(|w| w.r_neutral.unwrap_or(-1.0))
            .collect();
        assert_eq!(neutrals, [-1.0, -1.0, 0.0], "{class}");
    }
}

#[test]
fn changing_winding_count_resets_the_active_selector() {
    for class in ["transformer", "xfmrcode"] {
        for edit in [" ".to_owned(), format!("\nedit {class}.t ")] {
            let mut script = format!(
                "new circuit.c\nnew {class}.t windings=3 kvs=[115 12.47 4.16]{edit}windings=2 kv=13.8\n"
            );
            if class == "xfmrcode" {
                script.push_str("new transformer.t xfmrcode=t buses=[a b]\n");
            } else {
                script.push_str("edit transformer.t buses=[a b]\n");
            }
            let t = transformer(&script);
            assert_eq!(t.windings.len(), 2);
            assert_eq!(t.windings[0].v_ref, 13_800.0, "{class}");
            assert_eq!(t.windings[1].v_ref, 12_470.0, "{class}");
        }
    }
}
