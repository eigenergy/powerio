//! `LogBuf`: thread safe ring buffer wired to a `tracing` `MakeWriter`.
//! All log records anywhere in the program land here so the TUI log pane
//! can render them.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};

const CAPACITY: usize = 1024;

#[derive(Clone, Debug)]
pub struct LogBuf {
    inner: Arc<Mutex<VecDeque<String>>>,
}

impl Default for LogBuf {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(CAPACITY))),
        }
    }
}

impl LogBuf {
    pub fn push(&self, line: impl Into<String>) {
        let mut g = self.inner.lock().expect("log mutex poisoned");
        if g.len() == CAPACITY {
            g.pop_front();
        }
        g.push_back(line.into());
    }

    pub fn push_parse_warnings(&self, path: &std::path::Path, warnings: &[String]) {
        for w in warnings {
            self.push(format!("WARN  parse {}: {w}", path.display()));
        }
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("log mutex poisoned")
            .iter()
            .cloned()
            .collect()
    }

    pub fn writer(&self) -> LogWriter {
        LogWriter {
            buf: self.clone(),
            partial: Vec::new(),
        }
    }
}

/// `tracing-subscriber` writes records to this. We split on `\n` to keep
/// the ring buffer line oriented.
pub struct LogWriter {
    buf: LogBuf,
    partial: Vec<u8>,
}

impl io::Write for LogWriter {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        for &b in src {
            if b == b'\n' {
                let line = String::from_utf8_lossy(&self.partial)
                    .trim_end()
                    .to_string();
                if !line.is_empty() {
                    self.buf.push(line);
                }
                self.partial.clear();
            } else {
                self.partial.push(b);
            }
        }
        Ok(src.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.writer()
    }
}
