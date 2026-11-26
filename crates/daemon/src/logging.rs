use latch_core::config::Paths;
use latch_core::redact::redact;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::DirBuilderExt;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Pembungkus `MakeWriter` yang menyaring rahasia sebelum menulis ke berkas log atau stderr.
#[derive(Clone)]
pub struct RedactingMakeWriter<M> {
    inner: M,
}

impl<M> RedactingMakeWriter<M> {
    #[must_use]
    pub const fn new(inner: M) -> Self {
        Self { inner }
    }
}

pub struct RedactingWriter<W> {
    inner: W,
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let safe = redact(&text);
        self.inner.write_all(safe.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<'a, M> MakeWriter<'a> for RedactingMakeWriter<M>
where
    M: MakeWriter<'a>,
{
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter {
            inner: self.inner.make_writer(),
        }
    }
}

/// Menginisialisasi `tracing` dengan rolling file harian dan redaksi pola rahasia.
pub fn init(paths: &Paths, foreground: bool) -> io::Result<WorkerGuard> {
    let logs_dir = paths.logs_dir();
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&logs_dir)?;

    let file_appender = tracing_appender::rolling::daily(&logs_dir, "daemon.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_env("LATCH_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(RedactingMakeWriter::new(non_blocking));

    let registry = tracing_subscriber::registry().with(filter).with(file_layer);

    if foreground {
        let stderr_layer =
            tracing_subscriber::fmt::layer().with_writer(RedactingMakeWriter::new(io::stderr));
        let _ = registry.with(stderr_layer).try_init();
    } else {
        let _ = registry.try_init();
    }

    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    #[derive(Clone)]
    struct BufMakeWriter(Arc<Mutex<Vec<u8>>>);

    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for BufMakeWriter {
        type Writer = BufWriter;

        fn make_writer(&'a self) -> Self::Writer {
            BufWriter(Arc::clone(&self.0))
        }
    }

    #[test]
    fn redacting_writer_masks_canary_secret() {
        let dir = TempDir::new().unwrap();
        let out_path = dir.path().join("test.log");
        let buf = Arc::new(Mutex::new(Vec::new()));
        let mk = RedactingMakeWriter::new(BufMakeWriter(Arc::clone(&buf)));

        let mut w = mk.make_writer();
        writeln!(w, "auth failed for sk-ant-CANARY1234567890abcdef123").unwrap();
        w.flush().unwrap();

        let bytes = buf.lock().unwrap().clone();
        fs::write(&out_path, &bytes).unwrap();
        let content = fs::read_to_string(&out_path).unwrap();
        assert!(content.contains("[REDACTED]"));
        assert!(!content.contains("CANARY1234567890"));
    }
}
