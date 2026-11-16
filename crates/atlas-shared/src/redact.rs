//! Scrubs anything that looks like a credential out of text.
//!
//! Runs on every log line and every error detail. It errs on the side of
//! hiding too much: an ugly log line is fine, a leaked token isn't.

use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

pub const REDACTED: &str = "[REDACTED]";

static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let p = |s: &str| Regex::new(s).expect("static redaction regex");
    vec![
        // JSON / query-string style secrets: "access_token":"...", refresh_token=...
        (
            p(r#"(?i)("?(?:access_token|refresh_token|id_token|accessToken|clientToken|xuid|uhs|token|password|secret|code)"?\s*[:=]\s*"?)([^"&\s,}]+)"#),
            "${1}[REDACTED]",
        ),
        // HTTP Authorization headers
        (p(r"(?i)(authorization:\s*(?:bearer|xbl3\.0)\s+)\S+"), "${1}[REDACTED]"),
        (p(r"(?i)(bearer\s+)[A-Za-z0-9\-_.~+/=]{8,}"), "${1}[REDACTED]"),
        // JWTs anywhere
        (p(r"eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}"), REDACTED),
        // Minecraft launch argument --accessToken <value>
        (p(r"(?i)(--accessToken\s+)\S+"), "${1}[REDACTED]"),
        // Microsoft refresh tokens (M.C5xx_BAY... style)
        (p(r"M\.[A-Z0-9]{2,6}_[A-Za-z0-9!*$_\-.]{20,}"), REDACTED),
    ]
});

/// Return `input` with anything that looks like a credential replaced by
/// `[REDACTED]`. Borrowed when nothing matched.
pub fn redact(input: &str) -> Cow<'_, str> {
    let mut out: Cow<'_, str> = Cow::Borrowed(input);
    for (re, rep) in PATTERNS.iter() {
        if re.is_match(&out) {
            out = Cow::Owned(re.replace_all(&out, *rep).into_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_plain_text_alone() {
        let s = "Downloaded 42 files to /home/a/.local/share/atlaslauncher";
        assert!(matches!(redact(s), Cow::Borrowed(_)));
    }

    #[test]
    fn redacts_json_tokens() {
        let s = r#"{"access_token":"abc123secret","expires_in":3600}"#;
        let r = redact(s);
        assert!(!r.contains("abc123secret"), "{r}");
        assert!(r.contains("expires_in"));
    }

    #[test]
    fn redacts_query_tokens_and_headers() {
        assert!(!redact("refresh_token=M.R3_BAY.zzzzzz&x=1").contains("zzzzzz"));
        assert!(!redact("Authorization: Bearer abcdefghijklmnop").contains("abcdefghijklmnop"));
        assert!(!redact("Authorization: XBL3.0 x=1;abcdef").contains("abcdef"));
    }

    #[test]
    fn redacts_launch_args_and_jwts() {
        let r = redact("java -jar mc.jar --accessToken eyJaaaaaa.bbbbbbbb.cccccccc --version 1.21");
        assert!(!r.contains("eyJ"), "{r}");
        assert!(r.contains("--version 1.21"));
    }

    #[test]
    fn redacts_ms_refresh_tokens() {
        let r = redact("got M.C539_BAY.0.U.-CkY2dVhhZXlnQmVXVlpZbWU1Z1FQRDdGTUtLTA ok");
        assert!(!r.contains("CkY2"), "{r}");
    }
}
