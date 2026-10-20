//! Java discovery helpers. Pure functions so they run under `cargo test` on the host.

use zed_extension_api as zed;

/// The daemon is compiled for Java 21 (`jvmToolchain(21)` in brewery-daemon/build.gradle.kts).
pub const MIN_MAJOR: u32 = 21;

/// The major version from `java -version` output (printed on stderr). `1.8` style versions report 8.
pub fn parse_major(version_output: &str) -> Option<u32> {
    let start = version_output.find("version \"")? + "version \"".len();
    let rest = &version_output[start..];
    let version = &rest[..rest.find('"')?];
    let mut parts = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty());
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Download URL of the newest Temurin 21 JRE for this platform.
pub fn adoptium_url(os: zed::Os, arch: zed::Architecture) -> Result<String, String> {
    let os = match os {
        zed::Os::Mac => "mac",
        zed::Os::Linux => "linux",
        zed::Os::Windows => "windows",
    };
    let arch = match arch {
        zed::Architecture::Aarch64 => "aarch64",
        zed::Architecture::X8664 => "x64",
        zed::Architecture::X86 => {
            return Err("Brewery Lite needs a 64-bit system for Java 21".into());
        }
    };
    Ok(format!(
        "https://api.adoptium.net/v3/binary/latest/{MIN_MAJOR}/ga/{os}/{arch}/jre/hotspot/normal/eclipse"
    ))
}

/// The `java` executable inside an extracted Temurin archive, given the extraction dir's entries.
pub fn java_in_jre(entries: &[String], os: zed::Os) -> Option<String> {
    let top = entries.iter().find(|e| e.starts_with("jdk"))?;
    Some(match os {
        zed::Os::Mac => format!("{top}/Contents/Home/bin/java"),
        zed::Os::Windows => format!("{top}/bin/java.exe"),
        zed::Os::Linux => format!("{top}/bin/java"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_java_version_output() {
        assert_eq!(
            parse_major("openjdk version \"21.0.5\" 2024-10-15 LTS\nOpenJDK Runtime Environment"),
            Some(21)
        );
        assert_eq!(
            parse_major("java version \"1.8.0_412\"\nJava(TM) SE Runtime"),
            Some(8)
        );
        assert_eq!(
            parse_major("openjdk version \"25-ea\" 2025-09-16"),
            Some(25)
        );
        assert_eq!(parse_major("openjdk version \"17\" 2021-09-14"), Some(17));
        assert_eq!(parse_major("'java' is not recognized"), None);
    }

    #[test]
    fn adoptium_urls_per_platform() {
        assert_eq!(
            adoptium_url(zed::Os::Mac, zed::Architecture::Aarch64).unwrap(),
            "https://api.adoptium.net/v3/binary/latest/21/ga/mac/aarch64/jre/hotspot/normal/eclipse"
        );
        assert_eq!(
            adoptium_url(zed::Os::Windows, zed::Architecture::X8664).unwrap(),
            "https://api.adoptium.net/v3/binary/latest/21/ga/windows/x64/jre/hotspot/normal/eclipse"
        );
        assert!(adoptium_url(zed::Os::Linux, zed::Architecture::X86).is_err());
    }

    #[test]
    fn finds_java_in_extracted_jre() {
        let entries = vec!["jdk-21.0.5+11-jre".to_string()];
        assert_eq!(
            java_in_jre(&entries, zed::Os::Linux).as_deref(),
            Some("jdk-21.0.5+11-jre/bin/java")
        );
        assert_eq!(
            java_in_jre(&entries, zed::Os::Windows).as_deref(),
            Some("jdk-21.0.5+11-jre/bin/java.exe")
        );
        assert_eq!(
            java_in_jre(&entries, zed::Os::Mac).as_deref(),
            Some("jdk-21.0.5+11-jre/Contents/Home/bin/java")
        );
        assert_eq!(java_in_jre(&[], zed::Os::Linux), None);
    }
}
