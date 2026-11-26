use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Ring buffer berkapasitas tetap untuk menyimpan byte output PTY terbaru di RAM.
pub struct RingBuffer {
    cap: usize,
    buf: VecDeque<u8>,
}

impl RingBuffer {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            buf: VecDeque::with_capacity(cap.min(64 * 1024)),
        }
    }

    pub fn push_slice(&mut self, data: &[u8]) {
        if self.cap == 0 {
            return;
        }
        if data.len() >= self.cap {
            self.buf.clear();
            let start = data.len() - self.cap;
            self.buf.extend(&data[start..]);
            return;
        }
        let overflow = (self.buf.len() + data.len()).saturating_sub(self.cap);
        for _ in 0..overflow {
            self.buf.pop_front();
        }
        self.buf.extend(data);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }
}

/// Penulis log PTY ke disk dengan rotasi ukuran file.
pub struct RollingLog {
    path: PathBuf,
    max_bytes: u64,
    max_files: usize,
    current_bytes: u64,
    file: Option<File>,
}

impl RollingLog {
    pub fn open(path: PathBuf, max_bytes: u64, max_files: usize) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let current_bytes = file.metadata()?.len();
        Ok(Self {
            path,
            max_bytes,
            max_files: max_files.max(1),
            current_bytes,
            file: Some(file),
        })
    }

    pub fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        if self.max_bytes > 0 && self.current_bytes + data.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        if let Some(file) = self.file.as_mut() {
            file.write_all(data)?;
            self.current_bytes += data.len() as u64;
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.take();
        for idx in (1..self.max_files).rev() {
            let src = rotated_path(&self.path, idx);
            let dst = rotated_path(&self.path, idx + 1);
            if src.exists() {
                fs::rename(&src, &dst)?;
            }
        }
        if self.path.exists() {
            fs::rename(&self.path, rotated_path(&self.path, 1))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        self.current_bytes = 0;
        self.file = Some(file);
        Ok(())
    }
}

fn rotated_path(base: &Path, index: usize) -> PathBuf {
    let name = base.file_name().map_or_else(
        || "pty.log".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    base.with_file_name(format!("{name}.{index}"))
}
