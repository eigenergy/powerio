//! Non-regular filesystem entries on the DSS read paths: a redirect naming a
//! named pipe is refused promptly, without the parse waiting on a writer.

mod helpers;

#[cfg(unix)]
#[test]
fn a_redirect_to_a_named_pipe_is_refused_promptly() {
    use std::os::unix::ffi::OsStrExt;

    let tmp = tempfile::tempdir().unwrap();
    let fifo = tmp.path().join("linecodes.dss");
    let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: the pointer references the NUL-terminated buffer owned by
    // `c_path`, which outlives the call.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) }, 0);
    let master = tmp.path().join("master.dss");
    std::fs::write(
        &master,
        "clear\nnew circuit.c basekv=12.47 bus1=src\nredirect linecodes.dss\n",
    )
    .unwrap();

    // No process is attached to the pipe, so a blocking acquisition would
    // hang; the worker plus bounded receive turns a regression into a test
    // failure rather than a hang.
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        sender.send(helpers::parse_dss_file(&master)).unwrap();
    });
    let outcome = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("parsing a case that redirects to a writerless pipe completes promptly");
    worker.join().unwrap();
    // The pipe is refused as an include, with the error finding a refused
    // include records; its content was never read.
    let parsed = outcome.unwrap();
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.code() == "READ.DSS.INCLUDE_REFUSED"),
        "{:?}",
        parsed.warnings
    );
    assert!(parsed.line_codes().is_empty());
}
