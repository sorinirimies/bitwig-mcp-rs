//! The Bitwig extension, bundled into the binary, and installing it.
//!
//! `cargo install bitwig-mcp-rs` is all a user needs: the extension
//! (`extension/bitwig-mcp-rs.bwextension`, built by `scripts/bridge.nu embed` and checked
//! against its Java sources by the quality gate) is embedded here and written into Bitwig's
//! Extensions folder by `bitwig-mcp-rs install-extension`, or automatically when the server
//! starts.

use std::path::{Path, PathBuf};

/// The extension file, as built from `bitwig-bridge/`.
pub const BWEXTENSION: &[u8] = include_bytes!("../extension/bitwig-mcp-rs.bwextension");

/// File name inside Bitwig's Extensions folder.
pub const FILE_NAME: &str = "bitwig-mcp-rs.bwextension";

/// The one manual step, for messages.
pub const ADD_CONTROLLER: &str = "In Bitwig: Settings → Controllers → + Add Controller → \
vendor \"bitwig-mcp-rs\", product \"MCP bridge\" (restart Bitwig if it is not listed or after \
updating). Pick \"MCP bridge + SysEx out\" instead only to send SysEx to hardware synths.";

/// Bitwig's user folder on an OS (`linux`, `macos`, `windows`), given the home directory.
#[must_use]
pub fn bitwig_dir(os: &str, home: &Path) -> Option<PathBuf> {
    match os {
        "linux" => Some(home.join("Bitwig Studio")),
        "macos" | "windows" => Some(home.join("Documents").join("Bitwig Studio")),
        _ => None,
    }
}

/// What [`install`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The installed file was already this version.
    Current(PathBuf),
    /// Written for the first time.
    Installed(PathBuf),
    /// Replaced an older build (and removed other copies).
    Updated(PathBuf),
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Current(p) => write!(f, "the Bitwig extension is current: {}", p.display()),
            Self::Installed(p) => write!(f, "installed the Bitwig extension: {}", p.display()),
            Self::Updated(p) => write!(f, "updated the Bitwig extension: {}", p.display()),
        }
    }
}

/// Writes `bytes` as `dir/bitwig-mcp-rs.bwextension` unless identical, and removes other
/// copies (`bitwig-mcp-rs*.bwextension`, e.g. a downloaded release asset) so Bitwig does
/// not see the same extension twice. Creates `dir` when missing.
///
/// # Errors
/// File system errors, with the path.
pub fn install(dir: &Path, bytes: &[u8]) -> std::io::Result<Outcome> {
    let with_path = |e: std::io::Error, p: &Path| {
        std::io::Error::new(e.kind(), format!("{}: {e}", p.display()))
    };
    std::fs::create_dir_all(dir).map_err(|e| with_path(e, dir))?;
    let target = dir.join(FILE_NAME);
    let mut removed = false;
    for entry in std::fs::read_dir(dir)
        .map_err(|e| with_path(e, dir))?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != FILE_NAME && name.starts_with("bitwig-mcp-rs") && name.ends_with(".bwextension")
        {
            std::fs::remove_file(entry.path()).map_err(|e| with_path(e, &entry.path()))?;
            removed = true;
        }
    }
    let existing = std::fs::read(&target).ok();
    if existing.as_deref() == Some(bytes) {
        return Ok(if removed {
            Outcome::Updated(target)
        } else {
            Outcome::Current(target)
        });
    }
    // write next to it, then rename: Bitwig watches the folder and must never see half a jar
    let tmp = dir.join(format!(".{FILE_NAME}.tmp"));
    std::fs::write(&tmp, bytes).map_err(|e| with_path(e, &tmp))?;
    std::fs::rename(&tmp, &target).map_err(|e| with_path(e, &target))?;
    Ok(if existing.is_some() || removed {
        Outcome::Updated(target)
    } else {
        Outcome::Installed(target)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bwmcp-ext-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        d
    }

    #[test]
    fn the_embedded_extension_is_a_jar_with_our_definition() {
        assert_eq!(&BWEXTENSION[..2], b"PK", "a zip file");
        let needle = b"io/github/sorinirimies/bitwigmcp/BridgeExtensionDefinition";
        assert!(
            BWEXTENSION.windows(needle.len()).any(|w| w == needle),
            "contains the extension definition"
        );
    }

    #[test]
    fn bitwig_folders_per_os() {
        let home = Path::new("/home/u");
        assert_eq!(bitwig_dir("linux", home), Some(home.join("Bitwig Studio")));
        assert_eq!(
            bitwig_dir("macos", home),
            Some(home.join("Documents").join("Bitwig Studio"))
        );
        assert_eq!(bitwig_dir("plan9", home), None);
    }

    #[test]
    fn installs_updates_and_leaves_current_files_alone() {
        let dir = temp("install");
        let first = install(&dir, b"v1").expect("install");
        assert!(matches!(first, Outcome::Installed(_)), "{first:?}");
        assert!(matches!(install(&dir, b"v1"), Ok(Outcome::Current(_))));
        assert!(matches!(install(&dir, b"v2"), Ok(Outcome::Updated(_))));
        assert_eq!(std::fs::read(dir.join(FILE_NAME)).expect("read"), b"v2");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).expect("dir").flatten().collect();
        assert_eq!(leftovers.len(), 1, "no temp file left");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn removes_other_copies_but_not_other_extensions() {
        let dir = temp("copies");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("bitwig-mcp-rs-bridge-0.1.0.bwextension"), b"old").expect("w");
        std::fs::write(dir.join("DrivenByMoss.bwextension"), b"other").expect("w");
        assert!(matches!(install(&dir, b"new"), Ok(Outcome::Updated(_))));
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["DrivenByMoss.bwextension", FILE_NAME]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
