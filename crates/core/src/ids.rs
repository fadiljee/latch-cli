use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Identifier proyek berbasis UUID v7.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    /// Membuat `ProjectId` baru menggunakan UUID v7.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string())
    }

    /// Membentuk `ProjectId` dari string yang sudah ada.
    #[must_use]
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Mengembalikan 8 karakter hex terakhir tanpa tanda hubung.
    /// UUID v7 menyimpan timestamp di awal, sehingga sufiks lebih unik untuk tampilan ringkas.
    #[must_use]
    pub fn short(&self) -> String {
        short_hex_suffix(&self.0)
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifier sesi berbasis UUID v7.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Membuat `SessionId` baru menggunakan UUID v7.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string())
    }

    /// Membentuk `SessionId` dari string yang sudah ada.
    #[must_use]
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Mengembalikan 8 karakter hex terakhir tanpa tanda hubung.
    #[must_use]
    pub fn short(&self) -> String {
        short_hex_suffix(&self.0)
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn short_hex_suffix(raw: &str) -> String {
    let clean: String = raw.chars().filter(|c| *c != '-').collect();
    let len = clean.len();
    if len <= 8 {
        clean
    } else {
        clean[len - 8..].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_short_takes_last_eight_hex_chars() {
        let id = SessionId::from_string("0193a8b0-1234-7000-8000-abcdef012345");
        assert_eq!(id.short(), "ef012345");
        assert_eq!(id.short().len(), 8);
    }
}
