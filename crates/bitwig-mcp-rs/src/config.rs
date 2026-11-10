//! Settings from the command line and the environment. Pure: both are passed in.

use std::fmt;

/// Help text printed for `--help`.
pub const HELP: &str = "\
bitwig-mcp-rs - MCP server that controls Bitwig Studio over OSC (DrivenByMoss)

Usage: bitwig-mcp-rs [OPTIONS]
  Speaks MCP on stdin/stdout. Logs go to stderr (RUST_LOG=debug for more).

Options (each also reads the environment variable in brackets):
      --host <HOST>          Bitwig host                      [BITWIG_HOST, 127.0.0.1]
      --send-port <PORT>     Port Bitwig listens on           [BITWIG_SEND_PORT, 8000]
      --listen-port <PORT>   Port Bitwig sends to (we bind)   [BITWIG_LISTEN_PORT, 9000]
      --resolution <N>       DrivenByMoss value resolution    [BITWIG_RESOLUTION, 128]
      --bank-size <N>        DrivenByMoss bank page size      [BITWIG_BANK_SIZE, 8]
  -h, --help                 Print help
  -V, --version              Print version";

/// Connection settings. Defaults match the DrivenByMoss OSC controller defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
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

impl Config {
    /// Builds the settings: environment first, then flags override.
    ///
    /// # Errors
    /// [`UsageError`] for an unknown flag, a missing or unparsable value, or a zero
    /// resolution / bank size.
    pub fn parse<I, E>(args: I, env: E) -> Result<Cli, UsageError>
    where
        I: IntoIterator<Item = String>,
        E: Fn(&str) -> Option<String>,
    {
        let mut cfg = Self::default();
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

        let mut it = args.into_iter();
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
        Config::parse(args.iter().map(|s| (*s).to_owned()), no_env)
    }

    #[test]
    fn defaults() {
        assert_eq!(parse(&[]), Ok(Cli::Run(Config::default())));
    }

    #[test]
    fn flags_override_env() {
        let env = |k: &str| (k == "BITWIG_SEND_PORT").then(|| "1111".to_owned());
        let cli = Config::parse(["--send-port".to_owned(), "2222".to_owned()], env);
        let Ok(Cli::Run(cfg)) = cli else {
            panic!("expected Run, got {cli:?}")
        };
        assert_eq!(cfg.send_port, 2222);
    }

    #[test]
    fn env_is_used_when_no_flag() {
        let env = |k: &str| (k == "BITWIG_HOST").then(|| "10.0.0.2".to_owned());
        let cli = Config::parse(Vec::new(), env);
        let Ok(Cli::Run(cfg)) = cli else {
            panic!("expected Run, got {cli:?}")
        };
        assert_eq!(cfg.host, "10.0.0.2");
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
