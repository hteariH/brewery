//! Daemon jar helpers: release asset names and checksum checks.

use sha2::{Digest, Sha256};

pub fn asset_name(version: &str) -> String {
    format!("brewery-daemon-{version}.jar")
}

/// The digest `SHA256SUMS` lists for `name` (lowercase), in `sha256sum` format (`<hex>  <name>` or `<hex> *<name>`).
pub fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hex, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name).then(|| hex.to_ascii_lowercase())
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Daemon jars of other versions, left behind by earlier extension versions.
pub fn stale_jars(entries: &[String], current: &str) -> Vec<String> {
    entries
        .iter()
        .filter(|e| {
            e.starts_with("brewery-daemon-") && e.ends_with(".jar") && e.as_str() != current
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_and_checksums() {
        assert_eq!(asset_name("0.1.6"), "brewery-daemon-0.1.6.jar");
        let sums = "aaa  Brewery-0.1.6-x86_64-setup.exe\nBBB *brewery-daemon-0.1.6.jar\n";
        assert_eq!(
            expected_sha256(sums, "brewery-daemon-0.1.6.jar").as_deref(),
            Some("bbb")
        );
        assert_eq!(expected_sha256(sums, "missing.jar"), None);
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn stale_jars_are_other_daemon_versions() {
        let entries = vec![
            "brewery-daemon-0.1.5.jar".into(),
            "brewery-daemon-0.1.6.jar".into(),
            "jre".into(),
        ];
        assert_eq!(
            stale_jars(&entries, "brewery-daemon-0.1.6.jar"),
            vec!["brewery-daemon-0.1.5.jar".to_string()]
        );
    }
}
