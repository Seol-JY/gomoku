//! Local files under the config dir: `config.toml`, `credentials.toml`,
//! `state.toml`, `gomoku.log`

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Set by build.rs: release -> production, debug -> localhost, env override
pub const DEFAULT_SERVER: &str = env!("GOMOKU_DEFAULT_SERVER");

#[derive(Clone, Debug)]
pub struct Paths {
    dir: PathBuf,
}

impl Paths {
    /// `GOMOKU_CONFIG_DIR` > `~/.config/gomoku` (Unix, per the plan) > platform dir (Windows)
    pub fn resolve() -> Result<Self> {
        if let Some(dir) = std::env::var_os("GOMOKU_CONFIG_DIR") {
            return Ok(Self {
                dir: PathBuf::from(dir),
            });
        }
        #[cfg(windows)]
        {
            let pd = directories::ProjectDirs::from("", "", "gomoku")
                .context("cannot determine the config directory")?;
            Ok(Self {
                dir: pd.config_dir().to_path_buf(),
            })
        }
        #[cfg(not(windows))]
        {
            let base =
                directories::BaseDirs::new().context("cannot determine the home directory")?;
            Ok(Self {
                dir: base.home_dir().join(".config").join("gomoku"),
            })
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn config_file(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    pub fn credentials_file(&self) -> PathBuf {
        self.dir.join("credentials.toml")
    }

    pub fn state_file(&self) -> PathBuf {
        self.dir.join("state.toml")
    }

    pub fn log_file(&self) -> PathBuf {
        self.dir.join("gomoku.log")
    }

    pub fn ensure_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot create {}", self.dir.display()))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    pub server: String,
    pub user_id: String,
    pub token: String,
    pub display_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[allow(
    clippy::struct_field_names,
    reason = "file keys are part of the on-disk format"
)]
pub struct LocalState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_room_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_server: Option<String>,
}

#[derive(Debug)]
pub enum Loaded<T> {
    Missing,
    Corrupt(String),
    Ok(T),
}

fn load_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Loaded<T> {
    match fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Missing,
        Err(e) => Loaded::Corrupt(e.to_string()),
        Ok(text) => match toml::from_str(&text) {
            Ok(v) => Loaded::Ok(v),
            Err(e) => Loaded::Corrupt(e.to_string()),
        },
    }
}

fn save_toml<T: Serialize>(path: &Path, value: &T, private: bool) -> Result<()> {
    let text = toml::to_string_pretty(value).context("serialising toml")?;
    write_atomic(path, text.as_bytes(), private)
}

/// temp file + rename: a crash never leaves a half-written file
fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        std::io::Write::write_all(&mut f, bytes)?;
        std::io::Write::flush(&mut f)?;
    }
    fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}

impl Paths {
    pub fn load_config(&self) -> Loaded<Config> {
        load_toml(&self.config_file())
    }

    pub fn load_credentials(&self) -> Loaded<Credentials> {
        load_toml(&self.credentials_file())
    }

    pub fn save_credentials(&self, creds: &Credentials) -> Result<()> {
        save_toml(&self.credentials_file(), creds, true)
    }

    pub fn delete_credentials(&self) {
        let _ = fs::remove_file(self.credentials_file());
    }

    pub fn load_state(&self) -> LocalState {
        match load_toml(&self.state_file()) {
            Loaded::Ok(s) => s,
            _ => LocalState::default(),
        }
    }

    pub fn save_state(&self, state: &LocalState) -> Result<()> {
        save_toml(&self.state_file(), state, false)
    }
}

/// flag > env (folded into `flag` by clap) > config file > default
pub fn resolve_server(flag: Option<&str>, cfg: &Config) -> String {
    let raw = flag
        .map(str::to_owned)
        .or_else(|| cfg.server.clone())
        .unwrap_or_else(|| DEFAULT_SERVER.to_owned());
    normalize_server(&raw)
}

pub fn normalize_server(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('/');
    if s.contains("://") {
        s.to_owned()
    } else {
        format!("http://{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_precedence() {
        let cfg = Config {
            server: Some("http://cfg:1/".into()),
        };
        assert_eq!(resolve_server(Some("https://flag"), &cfg), "https://flag");
        assert_eq!(resolve_server(None, &cfg), "http://cfg:1");
        assert_eq!(resolve_server(None, &Config::default()), DEFAULT_SERVER);
        assert_eq!(
            normalize_server("example.com:3000/"),
            "http://example.com:3000"
        );
    }

    #[test]
    fn roundtrip_files() {
        let dir = std::env::temp_dir().join(format!("gomoku-test-{}", std::process::id()));
        let paths = Paths { dir: dir.clone() };
        let creds = Credentials {
            server: "http://x".into(),
            user_id: "u".into(),
            token: "t".into(),
            display_name: "n".into(),
        };
        paths.save_credentials(&creds).unwrap();
        match paths.load_credentials() {
            Loaded::Ok(c) => assert_eq!(c, creds),
            other => panic!("{other:?}"),
        }
        fs::write(paths.credentials_file(), "not toml = = =").unwrap();
        assert!(matches!(paths.load_credentials(), Loaded::Corrupt(_)));
        paths.delete_credentials();
        assert!(matches!(paths.load_credentials(), Loaded::Missing));
        let _ = fs::remove_dir_all(dir);
    }
}
