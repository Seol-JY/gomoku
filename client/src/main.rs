//! `gomoku` — play Gomoku (Renju rules) with a friend, in your terminal

#![forbid(unsafe_code)]

mod api;
mod config;
mod driver;
mod plain;
mod session;
mod sse;
mod tui;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::api::{Api, ClientError};
use crate::config::{Credentials, Loaded, Paths};
use crate::driver::{Driver, Exit};
use crate::session::Session;

/// Play Gomoku (Renju rules) with a friend, in your terminal
#[derive(Parser, Debug)]
#[command(name = "gomoku", version, about, long_about = None)]
struct Cli {
    /// Server URL (also GOMOKU_SERVER or `server` in config.toml)
    #[arg(long, env = "GOMOKU_SERVER", global = true, value_name = "URL")]
    server: Option<String>,

    /// Line-mode UI instead of the full-screen TUI (automatic when piped)
    #[arg(long, global = true)]
    plain: bool,

    /// More logging in gomoku.log (-v debug, -vv trace)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a room and print the invite code, then wait for your friend
    Invite,
    /// Join a friend's room with their invite code
    Join {
        /// Six-character invite code (case-insensitive)
        code: String,
    },
    /// Reconnect to your last room
    Rejoin,
    /// Show or change your display name
    Name {
        /// The new name (omit to show the current one)
        new_name: Option<String>,
    },
    /// Your identity on this server
    Whoami,
    /// Show the resolved configuration and file locations
    Config,
    /// Update gomoku to the latest release
    Update,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let paths = Paths::resolve()?;
    paths.ensure_dir()?;
    init_logging(&paths, cli.verbose)?;

    let cfg = match paths.load_config() {
        Loaded::Ok(c) => c,
        Loaded::Missing => config::Config::default(),
        Loaded::Corrupt(e) => {
            eprintln!(
                "warning: {} is unreadable ({e}); ignoring it",
                paths.config_file().display()
            );
            config::Config::default()
        }
    };
    let server = config::resolve_server(cli.server.as_deref(), &cfg);
    let interactive = std::io::stdin().is_terminal();
    let plain = cli.plain || !std::io::stdout().is_terminal();

    match cli.command {
        Some(Command::Config) => {
            println!("server:       {server}");
            println!("config dir:   {}", paths.dir().display());
            println!("config:       {}", paths.config_file().display());
            println!("credentials:  {}", paths.credentials_file().display());
            println!("state:        {}", paths.state_file().display());
            println!("log:          {}", paths.log_file().display());
            println!("client:       {}", api::CLIENT_VERSION);
            return Ok(ExitCode::SUCCESS);
        }
        Some(Command::Update) => return self_update().await,
        _ => {}
    }

    // reject bad codes before any network call
    let join_code = match &cli.command {
        Some(Command::Join { code }) => match proto::normalize_invite_code(code) {
            Some(c) => Some(c),
            None => bail!(
                "{code:?} is not a valid invite code: expected 6 letters/digits \
                 (0, O, 1, I and L are never used)"
            ),
        },
        _ => None,
    };

    let (api, creds) = match ensure_identity(&paths, &server, interactive).await {
        Ok(v) => v,
        Err(e) => return Ok(report(&e)),
    };
    update_hint(&api).await;

    let result: Result<ExitCode, ClientError> = match cli.command {
        None => {
            println!("You are {} on {}.", creds.display_name, server);
            println!();
            println!("  gomoku invite          create a room and get an invite code");
            println!("  gomoku join <CODE>     join a friend's room");
            println!("  gomoku rejoin          go back to your last game");
            println!("  gomoku name <NAME>     change your display name");
            println!("  gomoku --help          everything else");
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Whoami) => match api.me().await {
            Ok(me) => {
                println!("{} ({})", me.display_name, me.user_id);
                println!("server: {server}");
                Ok(ExitCode::SUCCESS)
            }
            Err(e) => Err(e),
        },
        Some(Command::Name { new_name: None }) => {
            println!("{}", creds.display_name);
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Name {
            new_name: Some(name),
        }) => match api.update_me(name.trim()).await {
            Ok(me) => {
                let mut c = creds.clone();
                c.display_name.clone_from(&me.display_name);
                paths.save_credentials(&c)?;
                println!("You are now {}.", me.display_name);
                Ok(ExitCode::SUCCESS)
            }
            Err(e) => Err(e),
        },
        Some(Command::Invite) => match api.create_room().await {
            Ok(resp) => {
                println!("Invite code: {}", resp.invite_code);
                println!("Your friend runs:  gomoku join {}", resp.invite_code);
                println!("You play {}. The code is valid for 24 hours.", resp.my_role);
                if copy_to_clipboard(&resp.invite_code) {
                    println!("(copied to the clipboard)");
                }
                let mut session = Session::new(
                    resp.room_id.clone(),
                    creds.user_id.clone(),
                    creds.display_name.clone(),
                    Some(resp.invite_code.clone()),
                );
                session.set_snapshot(resp.snapshot);
                play(api, session, paths, plain).await
            }
            Err(e) => Err(e),
        },
        Some(Command::Join { .. }) => {
            let code = join_code.expect("validated above");
            match api.join_by_code(&code).await {
                Ok(resp) => {
                    let mut session = Session::new(
                        resp.room_id.clone(),
                        creds.user_id.clone(),
                        creds.display_name.clone(),
                        resp.snapshot.invite_code.clone().or(Some(code)),
                    );
                    session.notice(format!("Joined as {}.", resp.my_role));
                    session.set_snapshot(resp.snapshot);
                    play(api, session, paths, plain).await
                }
                Err(e) => Err(e),
            }
        }
        Some(Command::Rejoin) => {
            let Some(room_id) = driver::last_room(&paths, &server) else {
                bail!("no previous room on {server}; use `gomoku invite` or `gomoku join <CODE>`");
            };
            match api.join_room(&room_id).await {
                Ok(resp) => {
                    let mut session = Session::new(
                        resp.room_id.clone(),
                        creds.user_id.clone(),
                        creds.display_name.clone(),
                        resp.snapshot.invite_code.clone(),
                    );
                    session.set_snapshot(resp.snapshot);
                    play(api, session, paths, plain).await
                }
                Err(e) => Err(e),
            }
        }
        Some(Command::Config | Command::Update) => unreachable!("handled above"),
    };

    Ok(result.unwrap_or_else(|e| report(&e)))
}

async fn play(
    api: Api,
    session: Session,
    paths: Paths,
    plain: bool,
) -> Result<ExitCode, ClientError> {
    let driver = Driver::new(api, session, paths);
    let exit = if plain {
        plain::run(driver).await
    } else {
        tui::run(driver).await
    };
    match exit {
        Ok(Exit::Normal) => {
            println!("Bye. Come back with `gomoku rejoin`.");
            Ok(ExitCode::SUCCESS)
        }
        Ok(Exit::Outdated { min }) => Err(ClientError::Outdated { min }),
        Ok(Exit::Fatal(msg)) => {
            eprintln!("error: {msg}");
            Ok(ExitCode::from(1))
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            Ok(ExitCode::from(1))
        }
    }
}

fn report(err: &ClientError) -> ExitCode {
    match err {
        ClientError::Outdated { min } => {
            eprintln!(
                "This gomoku ({}) is too old; the server requires {min} or newer.",
                api::CLIENT_VERSION
            );
            eprintln!("Update with `brew upgrade gomoku` or re-run the installer, then try again.");
            ExitCode::from(2)
        }
        ClientError::Network(msg) => {
            eprintln!("Cannot reach the gomoku server: {msg}");
            eprintln!(
                "Check the address (GOMOKU_SERVER, --server, or `server` in config.toml) and retry."
            );
            ExitCode::from(1)
        }
        other => {
            eprintln!("error: {other}");
            ExitCode::from(1)
        }
    }
}

/// UC-01: register anonymously when no credentials exist for `server`
async fn ensure_identity(
    paths: &Paths,
    server: &str,
    interactive: bool,
) -> Result<(Api, Credentials), ClientError> {
    let existing = match paths.load_credentials() {
        Loaded::Ok(c) if c.server == server => Some(c),
        Loaded::Ok(c) => {
            eprintln!(
                "Your saved identity is for {}; registering a new one on {server}.",
                c.server
            );
            None
        }
        Loaded::Corrupt(e) => {
            eprintln!("Credentials file is unreadable ({e}); discarding it and registering again.");
            paths.delete_credentials();
            None
        }
        Loaded::Missing => None,
    };
    if let Some(c) = existing {
        let api = Api::new(server, Some(c.token.clone()));
        match api.me().await {
            Ok(me) => {
                let mut c = c;
                c.display_name = me.display_name;
                return Ok((api, c));
            }
            Err(ClientError::Api(e)) if e.code == proto::ErrorCode::Unauthorized => {
                eprintln!("The server no longer knows this identity; registering again.");
                paths.delete_credentials();
            }
            Err(e) => return Err(e),
        }
    }

    let name = if interactive { prompt_name() } else { None };
    let api = Api::new(server, None);
    let resp = api.register(name).await?;
    let creds = Credentials {
        server: server.to_owned(),
        user_id: resp.user_id,
        token: resp.token,
        display_name: resp.display_name,
    };
    if let Err(e) = paths.save_credentials(&creds) {
        eprintln!("warning: could not save credentials: {e:#}");
    }
    println!(
        "Welcome, {}! Your identity is saved in {}.",
        creds.display_name,
        paths.credentials_file().display()
    );
    Ok((api.with_token(creds.token.clone()), creds))
}

/// Soft update: the server advertises the newest client; only a hint, never a block
async fn update_hint(api: &Api) {
    let Ok(h) = api.health().await else { return };
    if let Some(latest) = h.latest_client_version
        && proto::version_at_least(&latest, api::CLIENT_VERSION)
        && latest.trim() != api::CLIENT_VERSION
    {
        eprintln!(
            "gomoku {latest} is available (you have {}). Run `gomoku update`.",
            api::CLIENT_VERSION
        );
    }
}

/// Hard update path (UC-08): uses the install receipt written by the shell
/// installer or Homebrew, so `cargo install` builds fall back to instructions
async fn self_update() -> Result<ExitCode> {
    let mut updater = axoupdater::AxoUpdater::new_for("gomoku");
    // receipt of another install (installer, then brew) must not update the wrong copy
    let owned_by_receipt = updater.load_receipt().is_ok()
        && match (updater.install_prefix_root(), std::env::current_exe()) {
            (Ok(prefix), Ok(exe)) => exe.starts_with(prefix.as_std_path()),
            _ => false,
        };
    if !owned_by_receipt {
        println!(
            "This copy was not installed by the gomoku installer, so it cannot update itself."
        );
        println!("Use `brew upgrade gomoku` or re-run the installer from the latest release.");
        return Ok(ExitCode::from(1));
    }
    match updater.run().await {
        Ok(Some(done)) => {
            println!("Updated to {}.", done.new_version);
            Ok(ExitCode::SUCCESS)
        }
        Ok(None) => {
            println!(
                "gomoku {} is already the latest version.",
                api::CLIENT_VERSION
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            eprintln!("update failed: {e}");
            Ok(ExitCode::from(1))
        }
    }
}

fn prompt_name() -> Option<String> {
    print!("Choose a display name (Enter for a random one): ");
    std::io::stdout().flush().ok()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok()?;
    let name: String = line.trim().chars().take(proto::MAX_NAME_LEN).collect();
    if name.is_empty() { None } else { Some(name) }
}

fn copy_to_clipboard(text: &str) -> bool {
    match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_owned())) {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!(error = %e, "clipboard unavailable");
            false
        }
    }
}

fn init_logging(paths: &Paths, verbose: u8) -> Result<()> {
    use tracing_subscriber::EnvFilter;
    let default = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_env("GOMOKU_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new(default));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_file())
        .with_context(|| format!("cannot open {}", paths.log_file().display()))?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(false)
        .with_writer(std::sync::Mutex::new(file))
        .init();
    Ok(())
}
