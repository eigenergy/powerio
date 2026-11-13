use powerio_archive::stream::{DeclaredSizeFailure, read_bounded, read_declared_size};

#[test]
fn payload_caps_include_the_eof_probe_and_prefix() {
    assert_eq!(read_bounded(&mut &b"abc"[..], 3).unwrap(), b"abc");
    assert!(matches!(
        read_bounded(&mut &b"abcd"[..], 3),
        Err(DeclaredSizeFailure::Long)
    ));
    assert!(matches!(
        read_declared_size(&mut &b""[..], b"abcd", 3),
        Err(DeclaredSizeFailure::Long)
    ));
    assert!(matches!(
        read_declared_size(&mut &b"c"[..], b"ab", 4),
        Err(DeclaredSizeFailure::Short(3))
    ));
    assert_eq!(
        read_declared_size(&mut &b"c"[..], b"ab", 3).unwrap(),
        b"abc"
    );
}
