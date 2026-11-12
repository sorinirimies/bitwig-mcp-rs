//! Capturing Bitwig's audio output to a file (Linux: PipeWire's `pw-record`), and measuring it
//! (`ffmpeg`'s EBU R128 filter). The parsing and naming are pure; [`record`] and [`measure`]
//! run the tools.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Serialize;

/// The PipeWire node of Bitwig's audio engine.
pub const BITWIG_NODE: &str = "Bitwig Studio";

/// Output formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// 32-bit float WAV, as recorded.
    #[default]
    Wav,
    /// FLAC, 24 bit (needs ffmpeg).
    Flac,
    /// MP3, 320 kbit/s (needs ffmpeg).
    Mp3,
}

impl Format {
    /// File extension.
    #[must_use]
    pub fn ext(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
        }
    }
}

/// Loudness of a file (EBU R128).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
pub struct Loudness {
    /// Integrated loudness, LUFS.
    pub integrated_lufs: Option<f64>,
    /// Loudness range, LU.
    pub range_lu: Option<f64>,
    /// True peak, dBTP.
    pub true_peak_dbtp: Option<f64>,
}

impl Loudness {
    /// A short verdict for streaming-style targets (about -14 LUFS, peaks below -1 dBTP).
    #[must_use]
    pub fn verdict(&self) -> String {
        let mut out = Vec::new();
        match self.integrated_lufs {
            None => out.push("no loudness measured".to_owned()),
            Some(l) if l < -60.0 => {
                out.push("silent: is the transport playing and the track audible?".to_owned())
            }
            Some(l) if l < -20.0 => out.push(format!(
                "quiet ({l:.1} LUFS): master gain or limiter could bring it to -14…-9"
            )),
            Some(l) if l > -7.0 => {
                out.push(format!("very loud ({l:.1} LUFS): likely over-compressed"))
            }
            Some(l) => out.push(format!("{l:.1} LUFS")),
        }
        if let Some(p) = self.true_peak_dbtp
            && p > -1.0
        {
            out.push(format!(
                "true peak {p:.1} dBTP: above -1, add a limiter or lower the master"
            ));
        }
        out.join("; ")
    }
}

/// Reads the summary of `ffmpeg -af ebur128=peak=true`.
#[must_use]
pub fn parse_ebur128(stderr: &str) -> Loudness {
    let summary = stderr.rsplit("Summary:").next().unwrap_or_default();
    let value = |label: &str| -> Option<f64> {
        summary.lines().find_map(|l| {
            let l = l.trim();
            l.strip_prefix(label)?
                .split_whitespace()
                .next()?
                .parse::<f64>()
                .ok()
        })
    };
    Loudness {
        integrated_lufs: value("I:"),
        range_lu: value("LRA:"),
        true_peak_dbtp: value("Peak:"),
    }
}

/// A file name from a label: letters, digits, `-` and `_`; spaces become `-`.
#[must_use]
pub fn safe_name(label: &str) -> String {
    let s: String = label
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "bitwig".to_owned() } else { s }
}

/// Where exports go by default: the user's music folder, `bitwig-mcp-rs/`.
#[must_use]
pub fn default_dir() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("bitwig-mcp-rs")
}

/// Seconds since the Unix epoch, for unique names.
#[must_use]
pub fn stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Whether a program is on the PATH.
#[must_use]
pub fn have(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Records `node`'s output to a 32-bit float WAV while `during` runs, plus `tail` seconds.
///
/// # Errors
/// When `pw-record` is missing or fails.
pub async fn record<F, Fut>(node: &str, wav: &Path, tail: Duration, during: F) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    if !cfg!(target_os = "linux") {
        bail!("audio capture needs PipeWire (Linux); use Bitwig's Export Audio dialog instead");
    }
    if let Some(dir) = wav.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let mut child = tokio::process::Command::new("pw-record")
        .args([
            "--target",
            node,
            "--channels",
            "2",
            "--rate",
            "48000",
            "--format",
            "f32",
        ])
        .arg(wav)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("cannot start pw-record (install PipeWire's tools)")?;
    // let the stream connect before the music starts
    tokio::time::sleep(Duration::from_millis(400)).await;
    if let Ok(Some(status)) = child.try_wait() {
        bail!("pw-record stopped at once ({status}): is Bitwig running on PipeWire as {node:?}?");
    }
    let outcome = during().await;
    tokio::time::sleep(tail).await;
    // SIGINT lets pw-record finish the WAV header
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .status()
            .await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    outcome?;
    let size = std::fs::metadata(wav).map(|m| m.len()).unwrap_or(0);
    if size < 1024 {
        bail!("the capture is empty: is Bitwig's audio engine running on PipeWire as {node:?}?");
    }
    Ok(())
}

/// Converts (and trims `start_trim` seconds of lead-in) with ffmpeg; WAV stays as is when no
/// trimming is wanted.
///
/// # Errors
/// When ffmpeg is missing or fails.
pub async fn encode(wav: &Path, out: &Path, format: Format, start_trim: f64) -> Result<()> {
    if format == Format::Wav && start_trim <= 0.0 {
        if wav != out {
            std::fs::rename(wav, out)
                .with_context(|| format!("cannot move to {}", out.display()))?;
        }
        return Ok(());
    }
    if !have("ffmpeg") {
        bail!(
            "{} needs ffmpeg; the WAV is at {}",
            format.ext(),
            wav.display()
        );
    }
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-ss"])
        .arg(format!("{start_trim:.3}"))
        .arg("-i")
        .arg(wav);
    match format {
        Format::Wav => cmd.args(["-c:a", "pcm_f32le"]),
        Format::Flac => cmd.args(["-c:a", "flac", "-sample_fmt", "s32"]),
        Format::Mp3 => cmd.args(["-c:a", "libmp3lame", "-b:a", "320k"]),
    };
    let status = cmd.arg(out).status().await.context("cannot run ffmpeg")?;
    if !status.success() {
        bail!("ffmpeg failed ({status}) writing {}", out.display());
    }
    if wav != out {
        let _ = std::fs::remove_file(wav);
    }
    Ok(())
}

/// Loudness of a file, when ffmpeg is there.
pub async fn measure(file: &Path) -> Option<Loudness> {
    let out = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(file)
        .args(["-af", "ebur128=peak=true", "-f", "null", "-"])
        .output()
        .await
        .ok()?;
    Some(parse_ebur128(&String::from_utf8_lossy(&out.stderr)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMMARY: &str = "[Parsed_ebur128_0 @ 0x5]\n  t: 2.9 M: -47.1\n\
[Parsed_ebur128_0 @ 0x5] Summary:\n\n  Integrated loudness:\n    I:         -12.3 LUFS\n    Threshold: -22.6 LUFS\n\n  \
Loudness range:\n    LRA:         4.5 LU\n    Threshold: -32.4 LUFS\n    LRA low:   -15.1 LUFS\n\n  \
True peak:\n    Peak:       -0.4 dBFS\n";

    #[test]
    fn parses_the_summary() {
        let l = parse_ebur128(SUMMARY);
        assert_eq!(l.integrated_lufs, Some(-12.3));
        assert_eq!(l.range_lu, Some(4.5));
        assert_eq!(l.true_peak_dbtp, Some(-0.4));
        assert!(l.verdict().contains("true peak"), "{}", l.verdict());
        assert_eq!(parse_ebur128("garbage"), Loudness::default());
    }

    #[test]
    fn verdicts() {
        let v = |i: f64| {
            Loudness {
                integrated_lufs: Some(i),
                range_lu: None,
                true_peak_dbtp: Some(-3.0),
            }
            .verdict()
        };
        assert!(v(-70.0).contains("silent"));
        assert!(v(-30.0).contains("quiet"));
        assert!(v(-5.0).contains("loud"));
        assert_eq!(v(-12.0), "-12.0 LUFS");
        assert!(Loudness::default().verdict().contains("no loudness"));
    }

    #[test]
    fn names_and_formats() {
        assert_eq!(safe_name("Night Drive / Drop!"), "Night-Drive---Drop");
        assert_eq!(safe_name("  "), "bitwig");
        assert_eq!(Format::Flac.ext(), "flac");
        assert!(default_dir().ends_with("bitwig-mcp-rs"));
    }
}
