//! Settings from the command line and the environment. Pure: both are passed in.

use std::fmt;

/// Help text printed for `--help`.
pub const HELP: &str = "\
bitwig-mcp-rs - MCP server that controls Bitwig Studio

Usage: bitwig-mcp-rs [OPTIONS]
  Speaks MCP on stdin/stdout. Logs go to stderr (RUST_LOG=debug for more).
  At start it installs or updates the bundled Bitwig extension in Bitwig's Extensions
  folder (when Bitwig's user folder exists); then add the controller once in Bitwig:
  Settings -> Controllers -> + Add Controller -> bitwig-mcp-rs -> MCP bridge.

       bitwig-mcp-rs install-extension [--dir <DIR>]
  Installs the bundled Bitwig extension now (default: Bitwig's Extensions folder).

Options (each also reads the environment variable in brackets):
      --backend <B>          bridge (our extension) or osc     [BITWIG_BACKEND, bridge]
      --host <HOST>          Bitwig host                       [BITWIG_HOST, 127.0.0.1]
      --bridge-port <PORT>   Bridge extension TCP port         [BITWIG_BRIDGE_PORT, 8417]
      --instruments-dir <D>  Hardware instrument profiles      [BITWIG_INSTRUMENTS_DIR,
                    <config dir>/bitwig-mcp-rs/instruments: ~/.config on Linux,
                    ~/Library/Application Support on macOS, %APPDATA% on Windows]
      --no-install-extension Leave Bitwig's Extensions folder alone
                                                  [BITWIG_NO_INSTALL_EXTENSION]
  OSC backend (DrivenByMoss):
      --send-port <PORT>     Port Bitwig listens on            [BITWIG_SEND_PORT, 8000]
      --listen-port <PORT>   Port Bitwig sends to (we bind)    [BITWIG_LISTEN_PORT, 9000]
      --resolution <N>       DrivenByMoss value resolution     [BITWIG_RESOLUTION, 128]
      --bank-size <N>        DrivenByMoss bank page size       [BITWIG_BANK_SIZE, 8]
  -h, --help                 Print help
  -V, --version              Print version";

/// Which Bitwig extension to talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Our bridge extension over TCP.
    Bridge,
    /// DrivenByMoss over OSC/UDP.
    Osc,
}

/// Connection settings. OSC defaults match the DrivenByMoss OSC controller defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Backend.
    pub backend: Backend,
    /// Bridge extension TCP port.
    pub bridge_port: u16,
    /// Hardware instrument profile directory.
    pub instruments_dir: std::path::PathBuf,
    /// Bitwig's user folder (`~/Bitwig Studio` on Linux), when the home directory is known.
    pub bitwig_dir: Option<std::path::PathBuf>,
    /// Install or update the bundled extension at start.
    pub install_extension: bool,
    /// Host running Bitwig.
    pub host: String,
    /// UDP port Bitwig receives commands on.
    pub send_port: u16,
    /// UDP port we bind to receive Bitwig's state.
    pub listen_port: u16,
    /// DrivenByMoss "Value resolution"; values run `0..resolution`.
    pub resolution: u32,
    /// DrivenByMoss "Bank page size" (tracks, scenes and clip slots addressable at once).
    pub bank_size: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: Backend::Bridge,
            bridge_port: 8417,
            instruments_dir: std::path::PathBuf::from("instruments"),
            bitwig_dir: None,
            install_extension: true,
            host: "127.0.0.1".into(),
            send_port: 8000,
            listen_port: 9000,
            resolution: 128,
            bank_size: 8,
        }
    }
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    /// Run the server.
    Run(Config),
    /// Print the help text.
    Help,
    /// Print the version.
    Version,
    /// Install the bundled Bitwig extension into this folder, then exit.
    InstallExtension(Option<std::path::PathBuf>),
}

/// A bad flag or value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

fn number<T: std::str::FromStr>(name: &str, v: &str) -> Result<T, UsageError> {
    v.trim()
        .parse()
        .map_err(|_| UsageError(format!("invalid value for {name}: {v:?}")))
}

fn backend(v: &str) -> Result<Backend, UsageError> {
    match v.trim().to_lowercase().as_str() {
        "bridge" => Ok(Backend::Bridge),
        "osc" => Ok(Backend::Osc),
        _ => Err(UsageError(format!(
            "backend must be bridge or osc, not {v:?}"
        ))),
    }
}

/// The per-user folders the settings start from (from `dirs` in production).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dirs {
    /// The user's config folder (`~/.config`, `~/Library/Application Support`, `%APPDATA%`).
    pub config: Option<std::path::PathBuf>,
    /// Bitwig's user folder (`~/Bitwig Studio`, `Documents/Bitwig Studio`).
    pub bitwig: Option<std::path::PathBuf>,
}

impl Dirs {
    /// This machine's folders.
    #[must_use]
    pub fn system() -> Self {
        Self {
            config: dirs::config_dir(),
            bitwig: crate::extension::system_bitwig_dir(),
        }
    }
}

impl Config {
    /// Builds the settings: environment first, then flags override.
    ///
    /// # Errors
    /// [`UsageError`] for an unknown flag, a missing or unparsable value, or a zero
    /// resolution / bank size.
    pub fn parse<I, E>(args: I, env: E, dirs: &Dirs) -> Result<Cli, UsageError>
    where
        I: IntoIterator<Item = String>,
        E: Fn(&str) -> Option<String>,
    {
        let mut cfg = Self::default();
        cfg.bitwig_dir.clone_from(&dirs.bitwig);
        if env("BITWIG_NO_INSTALL_EXTENSION").is_some_and(|v| !v.is_empty() && v != "0") {
            cfg.install_extension = false;
        }
        if let Some(c) = &dirs.config {
            cfg.instruments_dir = c.join("bitwig-mcp-rs").join("instruments");
        }
        if let Some(v) = env("BITWIG_INSTRUMENTS_DIR") {
            cfg.instruments_dir = v.into();
        }
        if let Some(v) = env("BITWIG_BACKEND") {
            cfg.backend = backend(&v)?;
        }
        if let Some(v) = env("BITWIG_BRIDGE_PORT") {
            cfg.bridge_port = number("BITWIG_BRIDGE_PORT", &v)?;
        }
        if let Some(v) = env("BITWIG_HOST") {
            cfg.host = v;
        }
        if let Some(v) = env("BITWIG_SEND_PORT") {
            cfg.send_port = number("BITWIG_SEND_PORT", &v)?;
        }
        if let Some(v) = env("BITWIG_LISTEN_PORT") {
            cfg.listen_port = number("BITWIG_LISTEN_PORT", &v)?;
        }
        if let Some(v) = env("BITWIG_RESOLUTION") {
            cfg.resolution = number("BITWIG_RESOLUTION", &v)?;
        }
        if let Some(v) = env("BITWIG_BANK_SIZE") {
            cfg.bank_size = number("BITWIG_BANK_SIZE", &v)?;
        }

        let mut it = args.into_iter().peekable();
        if it.peek().is_some_and(|a| a == "install-extension") {
            it.next();
            return match (it.next().as_deref(), it.next(), it.next()) {
                (None, _, _) => Ok(Cli::InstallExtension(None)),
                (Some("--dir"), Some(d), None) => Ok(Cli::InstallExtension(Some(d.into()))),
                _ => Err(UsageError(
                    "usage: bitwig-mcp-rs install-extension [--dir <DIR>]".into(),
                )),
            };
        }
        while let Some(flag) = it.next() {
            if matches!(flag.as_str(), "-h" | "--help") {
                return Ok(Cli::Help);
            }
            if matches!(flag.as_str(), "-V" | "--version") {
                return Ok(Cli::Version);
            }
            let mut value = || {
                it.next()
                    .ok_or_else(|| UsageError(format!("{flag} needs a value")))
            };
            match flag.as_str() {
                "--host" => cfg.host = value()?,
                "--backend" => cfg.backend = backend(&value()?)?,
                "--bridge-port" => cfg.bridge_port = number(&flag, &value()?)?,
                "--instruments-dir" => cfg.instruments_dir = value()?.into(),
                "--no-install-extension" => cfg.install_extension = false,
                "--send-port" => cfg.send_port = number(&flag, &value()?)?,
                "--listen-port" => cfg.listen_port = number(&flag, &value()?)?,
                "--resolution" => cfg.resolution = number(&flag, &value()?)?,
                "--bank-size" => cfg.bank_size = number(&flag, &value()?)?,
                _ => return Err(UsageError(format!("unknown argument: {flag}"))),
            }
        }
        if cfg.resolution < 2 {
            return Err(UsageError("resolution must be at least 2".into()));
        }
        if cfg.bank_size == 0 {
            return Err(UsageError("bank size must be at least 1".into()));
        }
        Ok(Cli::Run(cfg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn parse(args: &[&str]) -> Result<Cli, UsageError> {
        Config::parse(
            args.iter().map(|s| (*s).to_owned()),
            no_env,
            &Dirs::default(),
        )
    }

    #[test]
    fn defaults() {
        assert_eq!(parse(&[]), Ok(Cli::Run(Config::default())));
    }

    #[test]
    fn backend_and_bridge_options() {
        let Ok(Cli::Run(c)) = parse(&[
            "--backend",
            "OSC",
            "--bridge-port",
            "9999",
            "--instruments-dir",
            "/x",
        ]) else {
            panic!("expected Run")
        };
        assert_eq!((c.backend, c.bridge_port), (Backend::Osc, 9999));
        assert_eq!(c.instruments_dir, std::path::PathBuf::from("/x"));
        assert!(parse(&["--backend", "midi"]).is_err());
    }

    #[test]
    fn folders_come_from_dirs_and_env_wins() {
        let dirs = Dirs {
            config: Some("/cfg".into()),
            bitwig: Some("/home/u/Bitwig Studio".into()),
        };
        let Ok(Cli::Run(c)) = Config::parse(Vec::new(), |_: &str| None, &dirs) else {
            panic!()
        };
        assert_eq!(
            c.instruments_dir,
            std::path::PathBuf::from("/cfg/bitwig-mcp-rs/instruments")
        );
        assert_eq!(c.bitwig_dir, dirs.bitwig);
        let env = |k: &str| (k == "BITWIG_INSTRUMENTS_DIR").then(|| "/mine".to_owned());
        let Ok(Cli::Run(c)) = Config::parse(Vec::new(), env, &dirs) else {
            panic!()
        };
        assert_eq!(c.instruments_dir, std::path::PathBuf::from("/mine"));
    }

    #[test]
    fn flags_override_env() {
        let env = |k: &str| (k == "BITWIG_SEND_PORT").then(|| "1111".to_owned());
        let cli = Config::parse(
            ["--send-port".to_owned(), "2222".to_owned()],
            env,
            &Dirs::default(),
        );
        let Ok(Cli::Run(cfg)) = cli else {
            panic!("expected Run, got {cli:?}")
        };
        assert_eq!(cfg.send_port, 2222);
    }

    #[test]
    fn env_is_used_when_no_flag() {
        let env = |k: &str| (k == "BITWIG_HOST").then(|| "10.0.0.2".to_owned());
        let cli = Config::parse(Vec::new(), env, &Dirs::default());
        let Ok(Cli::Run(cfg)) = cli else {
            panic!("expected Run, got {cli:?}")
        };
        assert_eq!(cfg.host, "10.0.0.2");
    }

    #[test]
    fn install_extension_subcommand() {
        assert_eq!(
            parse(&["install-extension"]),
            Ok(Cli::InstallExtension(None))
        );
        assert_eq!(
            parse(&["install-extension", "--dir", "/x"]),
            Ok(Cli::InstallExtension(Some("/x".into())))
        );
        assert!(parse(&["install-extension", "--dir"]).is_err());
        assert!(parse(&["install-extension", "extra"]).is_err());
    }

    #[test]
    fn extension_install_can_be_switched_off() {
        let Ok(Cli::Run(c)) = parse(&["--no-install-extension"]) else {
            panic!()
        };
        assert!(!c.install_extension);
        let env = |k: &str| (k == "BITWIG_NO_INSTALL_EXTENSION").then(|| "1".to_owned());
        let Ok(Cli::Run(c)) = Config::parse(Vec::new(), env, &Dirs::default()) else {
            panic!()
        };
        assert!(!c.install_extension);
        let Ok(Cli::Run(c)) = parse(&[]) else {
            panic!()
        };
        assert!(c.install_extension);
    }

    #[test]
    fn help_and_version() {
        assert_eq!(parse(&["-h"]), Ok(Cli::Help));
        assert_eq!(parse(&["--version"]), Ok(Cli::Version));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&["--host"]).is_err());
        assert!(parse(&["--send-port", "x"]).is_err());
        assert!(parse(&["--resolution", "1"]).is_err());
        assert!(parse(&["--bank-size", "0"]).is_err());
    }
}
