use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

static REDACT_REGEX: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        sk-ant-[A-Za-z0-9_-]{16,}
        | sk-[A-Za-z0-9_-]{16,}
        | AIza[0-9A-Za-z_-]{30,}
        | gh[pousr]_[A-Za-z0-9]{20,}
        | (?i)(authorization|x-api-key)\s*[:=]\s*\S+(?:\s+\S+)?
        | (?i)bearer\s+[A-Za-z0-9._~+/=-]{8,}
        ",
    )
    .ok()
});

/// Menyaring pola token, API key, dan header otorisasi dari string log atau audit.
#[must_use]
pub fn redact(input: &str) -> Cow<'_, str> {
    let Some(re) = REDACT_REGEX.as_ref() else {
        return Cow::Borrowed("[REDACTED]");
    };
    re.replace_all(input, "[REDACTED]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_canary_secrets() {
        let canaries = [
            "token sk-ant-api03-1234567890abcdef123456",
            "key=sk-1234567890abcdef123456",
            "google=AIzaSyDummyKey123456789012345678901234",
            "ghp_abcdefghijklmnopqrstuvwxyz1234",
            "Authorization: Bearer secret-token-value-12345",
            "x-api-key: my-very-secret-key",
        ];
        for sample in canaries {
            let out = redact(sample);
            assert!(out.contains("[REDACTED]"), "failed to redact: {sample}");
            assert!(!out.contains("1234567890"), "secret leaked in: {out}");
            assert!(!out.contains("my-very-secret-key"), "key leaked in: {out}");
        }
    }

    #[test]
    fn leaves_normal_text_untouched() {
        let msg = "session started in worktree /tmp/wt";
        assert!(matches!(redact(msg), Cow::Borrowed(_)));
        assert_eq!(redact(msg), msg);
    }
}
