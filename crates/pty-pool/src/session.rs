use crate::ring::{RingBuffer, RollingLog};
use latch_core::SpawnSpec;
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use portable_pty::{CommandBuilder, MasterPty, PtySize as NativePtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{broadcast, oneshot};

const BATCH_INTERVAL: Duration = Duration::from_millis(12);
const BROADCAST_CAP: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        Self {
            cols: 120,
            rows: 40,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PtyOptions {
    pub ring_bytes: usize,
    pub log_path: Option<PathBuf>,
    pub log_max_bytes: u64,
    pub log_max_files: usize,
}

impl Default for PtyOptions {
    fn default() -> Self {
        Self {
            ring_bytes: 1_048_576,
            log_path: None,
            log_max_bytes: 10_485_760,
            log_max_files: 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: i32,
    pub success: bool,
}

pub type ExitWatcher = oneshot::Receiver<ExitInfo>;

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("PTY error: {0}")]
    Pty(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("missing child PID")]
    MissingPid,

    #[error("signal error: {0}")]
    Signal(#[from] Errno),

    #[error("mutex poisoned")]
    Poisoned,
}

struct ParserState {
    parser: vt100::Parser,
    ring: RingBuffer,
}

/// Sesi PTY aktif dengan thread pembaca khusus, batching 12 ms, ring buffer, dan `vt100`.
pub struct PtySession {
    pid: u32,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    state: Arc<Mutex<ParserState>>,
    tx: broadcast::Sender<Vec<u8>>,
    exited: Arc<AtomicBool>,
}

impl PtySession {
    /// Menjalankan proses anak di dalam PTY baru dengan ukuran dan opsi yang ditentukan.
    pub fn spawn(
        spec: SpawnSpec,
        size: PtySize,
        opts: PtyOptions,
    ) -> Result<(Self, ExitWatcher), PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(NativePtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Pty(e.to_string()))?;

        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        cmd.env_clear();
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Pty(e.to_string()))?;
        drop(pair.slave);

        let pid = child.process_id().ok_or(PtyError::MissingPid)?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::Pty(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Pty(e.to_string()))?;

        let state = Arc::new(Mutex::new(ParserState {
            parser: vt100::Parser::new(size.rows, size.cols, 1000),
            ring: RingBuffer::new(opts.ring_bytes),
        }));
        let pending = Arc::new(Mutex::new(Vec::<u8>::new()));
        let eof = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = broadcast::channel(BROADCAST_CAP);

        let mut rolling_log = match opts.log_path {
            Some(path) => Some(RollingLog::open(
                path,
                opts.log_max_bytes,
                opts.log_max_files,
            )?),
            None => None,
        };

        // Thread 1: pembaca blocking dari master PTY.
        let pending_reader = Arc::clone(&pending);
        let eof_reader = Arc::clone(&eof);
        thread::Builder::new()
            .name(format!("pty-read-{pid}"))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            eof_reader.store(true, Ordering::Release);
                            break;
                        }
                        Ok(n) => {
                            if let Ok(mut guard) = pending_reader.lock() {
                                guard.extend_from_slice(&buf[..n]);
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            eof_reader.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
            })?;

        // Thread 2: flusher setiap 12 ms untuk batching output, parser vt100, ring buffer, dan log.
        let pending_flusher = Arc::clone(&pending);
        let eof_flusher = Arc::clone(&eof);
        let state_flusher = Arc::clone(&state);
        let tx_flusher = tx.clone();
        thread::Builder::new()
            .name(format!("pty-flush-{pid}"))
            .spawn(move || {
                loop {
                    thread::sleep(BATCH_INTERVAL);
                    let chunk = match pending_flusher.lock() {
                        Ok(mut guard) => {
                            if guard.is_empty() {
                                Vec::new()
                            } else {
                                std::mem::take(&mut *guard)
                            }
                        }
                        Err(_) => break,
                    };

                    if !chunk.is_empty() {
                        if let Ok(mut st) = state_flusher.lock() {
                            st.parser.process(&chunk);
                            st.ring.push_slice(&chunk);
                        }
                        if let Some(log) = rolling_log.as_mut()
                            && let Err(err) = log.write_all(&chunk)
                        {
                            tracing::warn!(pid, error = %err, "failed to write PTY rolling log");
                        }
                        let _ = tx_flusher.send(chunk);
                    } else if eof_flusher.load(Ordering::Acquire) {
                        break;
                    }
                }
            })?;

        // Thread 3: waiter untuk mendeteksi exit status proses anak.
        let (exit_tx, exit_rx) = oneshot::channel();
        let exited_waiter = Arc::clone(&exited);
        thread::Builder::new()
            .name(format!("pty-wait-{pid}"))
            .spawn(move || {
                let info = match child.wait() {
                    Ok(status) => ExitInfo {
                        code: i32::try_from(status.exit_code()).unwrap_or(1),
                        success: status.success(),
                    },
                    Err(_) => ExitInfo {
                        code: 1,
                        success: false,
                    },
                };
                exited_waiter.store(true, Ordering::Release);
                let _ = exit_tx.send(info);
            })?;

        Ok((
            Self {
                pid,
                master: Mutex::new(pair.master),
                writer: Mutex::new(writer),
                state,
                tx,
                exited,
            },
            exit_rx,
        ))
    }

    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// Mengambil snapshot layar terformat (`vt100`) dan berlangganan frame output berikutnya.
    pub fn subscribe(&self) -> Result<(Vec<u8>, broadcast::Receiver<Vec<u8>>), PtyError> {
        let guard = self.state.lock().map_err(|_| PtyError::Poisoned)?;
        let snapshot = guard.parser.screen().contents_formatted();
        let rx = self.tx.subscribe();
        Ok((snapshot, rx))
    }

    /// Mengambil teks layar saat ini untuk tampilan cepat.
    pub fn screen_text(&self) -> Result<String, PtyError> {
        let guard = self.state.lock().map_err(|_| PtyError::Poisoned)?;
        Ok(guard.parser.screen().contents())
    }

    /// Mengambil salinan isi ring buffer saat ini.
    pub fn ring_snapshot(&self) -> Result<Vec<u8>, PtyError> {
        let guard = self.state.lock().map_err(|_| PtyError::Poisoned)?;
        Ok(guard.ring.snapshot())
    }

    /// Mengirim input mentah ke master PTY.
    pub fn write_input(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut writer = self.writer.lock().map_err(|_| PtyError::Poisoned)?;
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    /// Menyelaraskan ukuran master PTY dan parser `vt100`.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        let master = self.master.lock().map_err(|_| PtyError::Poisoned)?;
        master
            .resize(NativePtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Pty(e.to_string()))?;
        drop(master);

        let mut st = self.state.lock().map_err(|_| PtyError::Poisoned)?;
        st.parser.screen_mut().set_size(rows, cols);
        Ok(())
    }

    /// Menjeda process group sesi dengan `SIGSTOP`.
    pub fn pause(&self) -> Result<(), PtyError> {
        send_pgid_signal(self.pid, Signal::SIGSTOP)
    }

    /// Melanjutkan process group sesi dengan `SIGCONT`.
    pub fn resume(&self) -> Result<(), PtyError> {
        send_pgid_signal(self.pid, Signal::SIGCONT)
    }

    /// Menghentikan process group sesi dengan `SIGTERM`, lalu `SIGKILL` setelah `grace`.
    pub fn terminate(&self, grace: Duration) -> Result<(), PtyError> {
        if self.exited.load(Ordering::Acquire) {
            return Ok(());
        }
        send_pgid_signal(self.pid, Signal::SIGTERM)?;

        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if self.exited.load(Ordering::Acquire) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        if !self.exited.load(Ordering::Acquire) {
            send_pgid_signal(self.pid, Signal::SIGKILL)?;
        }
        Ok(())
    }
}

fn send_pgid_signal(pid: u32, sig: Signal) -> Result<(), PtyError> {
    let raw = i32::try_from(pid).map_err(|_| PtyError::MissingPid)?;
    match killpg(Pid::from_raw(raw), sig) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(e) => Err(PtyError::Signal(e)),
    }
}
