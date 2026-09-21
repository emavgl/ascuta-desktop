//! QR pairing payload + token generation for the phone bridge.
//!
//! The desktop renders a QR that encodes a compact, versioned URI:
//!
//! ```text
//! ascuta://pair?v=1&host=192.168.1.42&port=8737&token=<hex>&name=<hostname>
//! ```
//!
//! The phone scans it, stores `{host, port, token}`, and sends every
//! transcript with `Authorization: Bearer <token>` (see `bridge`). The token
//! is 256-bit random hex, generated once and persisted in `settings.json`;
//! "unpair" just rotates it and re-renders the QR.
//!
//! Everything here is pure (`to_uri`/`parse_uri`/`render_unicode`) so the
//! headless `--check` gate can assert pairing round-trips without a camera.

use anyhow::{anyhow, Context, Result};
use qrcode::render::unicode;
use qrcode::QrCode;

/// Deep-link scheme the phone app registers / scans for.
pub const PAIR_SCHEME: &str = "ascuta";
pub const PAIR_HOST: &str = "pair";
pub const PAIR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub name: String,
}

impl Pairing {
    /// Compact URI encoded in the QR. Field values are percent-encoded so a
    /// hostname with spaces (or an IPv6 host) stays parseable.
    pub fn to_uri(&self) -> String {
        format!(
            "{PAIR_SCHEME}://{PAIR_HOST}?v={PAIR_VERSION}&host={}&port={}&token={}&name={}",
            encode_component(&self.host),
            self.port,
            encode_component(&self.token),
            encode_component(&self.name),
        )
    }

    pub fn parse_uri(uri: &str) -> Result<Self> {
        let rest = uri
            .strip_prefix(&format!("{PAIR_SCHEME}://{PAIR_HOST}?"))
            .ok_or_else(|| anyhow!("not an ascuta pairing URI: {uri:?}"))?;

        let mut host = None;
        let mut port = None;
        let mut token = None;
        let mut name = None;
        let mut version = PAIR_VERSION;

        for field in rest.split('&') {
            if field.is_empty() {
                continue;
            }
            let (key, value) = field
                .split_once('=')
                .ok_or_else(|| anyhow!("malformed pairing field {field:?}"))?;
            let value = decode_component(value)?;
            match key {
                "v" => version = value.parse().context("pairing version")?,
                "host" => host = Some(value),
                "port" => port = Some(value.parse::<u16>().context("pairing port")?),
                "token" => token = Some(value),
                "name" => name = Some(value),
                // Forward-compatible: ignore unknown fields.
                _ => {}
            }
        }

        if version != PAIR_VERSION {
            return Err(anyhow!(
                "unsupported pairing version {version} (expected {PAIR_VERSION})"
            ));
        }
        let host = host
            .filter(|h| !h.is_empty())
            .ok_or_else(|| anyhow!("missing host"))?;
        let port = port.ok_or_else(|| anyhow!("missing port"))?;
        let token = token
            .filter(|t| !t.is_empty())
            .ok_or_else(|| anyhow!("missing token"))?;
        let name = name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "computer".to_string());

        Ok(Self {
            host,
            port,
            token,
            name,
        })
    }

    /// Terminal QR: `Dense1x2` packs two rows per text line via half blocks.
    pub fn render_unicode(&self) -> Result<String> {
        let code = QrCode::new(self.to_uri().as_bytes()).context("build QR")?;
        Ok(code.render::<unicode::Dense1x2>().quiet_zone(true).build())
    }
}

/// 256-bit random token, lowercase hex (64 chars).
pub fn generate_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("random bytes for pairing token: {e}"))?;
    Ok(hex_encode(&bytes))
}

/// Constant-time comparison for bearer tokens (avoids leaking the token
/// length/prefix through timing). Length is not secret in our threat model.
pub fn tokens_match(expected: &str, presented: &str) -> bool {
    let a = expected.as_bytes();
    let b = presented.as_bytes();
    // An unconfigured bridge must never accept anyone.
    if a.is_empty() || a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// RFC 3986 unreserved stays literal; everything else is `%XX` (uppercase).
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode_component(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let h = bytes
                .get(i + 1)
                .and_then(|c| (*c as char).to_digit(16))
                .ok_or_else(|| anyhow!("bad percent-escape in {s:?}"))?;
            let l = bytes
                .get(i + 2)
                .and_then(|c| (*c as char).to_digit(16))
                .ok_or_else(|| anyhow!("bad percent-escape in {s:?}"))?;
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).context("pairing value not UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Pairing {
        Pairing {
            host: "192.168.1.42".into(),
            port: 8737,
            token: "deadbeef".into(),
            name: "studio-pc".into(),
        }
    }

    #[test]
    fn uri_round_trips() {
        let p = sample();
        let parsed = Pairing::parse_uri(&p.to_uri()).unwrap();
        assert_eq!(parsed, p);
    }

    #[test]
    fn uri_round_trips_names_with_spaces_and_ipv6() {
        let p = Pairing {
            host: "fe80::1".into(),
            port: 65535,
            token: "tok".into(),
            name: "my pc / desk".into(),
        };
        assert_eq!(Pairing::parse_uri(&p.to_uri()).unwrap(), p);
    }

    #[test]
    fn rejects_foreign_uri() {
        assert!(Pairing::parse_uri("https://example.com").is_err());
        assert!(Pairing::parse_uri("ascuta://pair?host=x").is_err());
        assert!(Pairing::parse_uri("ascuta://pair?host=x&port=notaport&token=t").is_err());
        assert!(Pairing::parse_uri("ascuta://pair?v=9&host=x&port=1&token=t").is_err());
    }

    #[test]
    fn missing_name_defaults() {
        let uri = "ascuta://pair?v=1&host=h&port=1&token=t";
        assert_eq!(Pairing::parse_uri(uri).unwrap().name, "computer");
    }

    #[test]
    fn token_is_64_hex_chars_and_unique() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn token_compare_is_exact() {
        assert!(tokens_match("abc", "abc"));
        assert!(!tokens_match("abc", "abd"));
        assert!(!tokens_match("abc", "ab"));
        assert!(!tokens_match("", ""));
    }

    #[test]
    fn qr_renders() {
        let qr = sample().render_unicode().unwrap();
        // Dense1x2 emits two rows per line and uses half-block glyphs.
        assert!(qr.contains('▀') || qr.contains('▄') || qr.contains('█'));
    }
}
