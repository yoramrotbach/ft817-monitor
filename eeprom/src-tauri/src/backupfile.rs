//! The .817ee backup file: JSON with a readable hex dump and a SHA-256 of the
//! raw bytes, so damaged or hand-edited files are detected before a restore.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FORMAT: &str = "817-eeprom-backup";

#[derive(Debug, Serialize, Deserialize)]
pub struct BackupFile {
    pub format: String,
    pub format_version: u32,
    pub created: String,
    pub app: String,
    #[serde(default)]
    pub note: String,
    pub length: usize,
    pub version_bytes: String,
    pub sha256: String,
    /// "AAAA: XX XX ... (16 bytes per line)"
    pub dump: Vec<String>,
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hex_dump(img: &[u8]) -> Vec<String> {
    img.chunks(16)
        .enumerate()
        .map(|(i, c)| {
            let bytes: Vec<String> = c.iter().map(|b| format!("{b:02X}")).collect();
            format!("{:04X}: {}", i * 16, bytes.join(" "))
        })
        .collect()
}

pub fn make(img: &[u8], created: &str, note: &str) -> BackupFile {
    BackupFile {
        format: FORMAT.into(),
        format_version: 1,
        created: created.into(),
        app: format!("817 EEPROM {}", env!("CARGO_PKG_VERSION")),
        note: note.into(),
        length: img.len(),
        version_bytes: format!("{:02X} {:02X}", img[4], img[5]),
        sha256: sha256_hex(img),
        dump: hex_dump(img),
    }
}

pub fn to_json(f: &BackupFile) -> String {
    serde_json::to_string_pretty(f).expect("serializable")
}

/// Parse and fully verify a backup file. Returns the image and the metadata.
pub fn parse(text: &str) -> Result<(Vec<u8>, BackupFile), String> {
    let f: BackupFile = serde_json::from_str(text).map_err(|e| format!("Not a valid backup file: {e}"))?;
    if f.format != FORMAT {
        return Err(format!("Unknown file format \"{}\".", f.format));
    }
    if f.format_version != 1 {
        return Err(format!("This backup uses format version {}, which this app doesn't know. Update 817 EEPROM.", f.format_version));
    }
    let mut img = Vec::with_capacity(f.length);
    for (n, line) in f.dump.iter().enumerate() {
        let (addr, rest) = line.split_once(':').ok_or_else(|| format!("Dump line {} is malformed.", n + 1))?;
        let addr = usize::from_str_radix(addr.trim(), 16).map_err(|_| format!("Bad address on dump line {}.", n + 1))?;
        if addr != img.len() {
            return Err(format!("Dump line {} starts at 0x{addr:04X}, expected 0x{:04X}.", n + 1, img.len()));
        }
        for tok in rest.split_whitespace() {
            img.push(u8::from_str_radix(tok, 16).map_err(|_| format!("Bad byte \"{tok}\" on dump line {}.", n + 1))?);
        }
    }
    if img.len() != f.length {
        return Err(format!("The file says {} bytes but contains {}.", f.length, img.len()));
    }
    if sha256_hex(&img) != f.sha256.to_lowercase() {
        return Err("The file's checksum doesn't match its contents. It is damaged or was edited, so it won't be used for a restore.".into());
    }
    Ok((img, f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper_detection() {
        let img: Vec<u8> = (0..0x1926u32).map(|i| (i * 7) as u8).collect();
        let text = to_json(&make(&img, "2026-11-25 12:00", "test"));
        let (back, meta) = parse(&text).unwrap();
        assert_eq!(back, img);
        assert_eq!(meta.length, 0x1926);
        let tampered = text.replacen("0010: ", "0010: FF ", 1).replacen(" 70 ", " 71 ", 1);
        assert!(parse(&tampered).is_err());
    }
}
