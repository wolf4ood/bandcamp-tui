//! bandcamp-tui: a terminal client for bandcamp.com.

mod api;
mod app;
mod config;
#[cfg(target_os = "linux")]
mod mpris;
mod player;
mod secrets;
mod ui;

use std::io::stdout;

use anyhow::{Context, Result};
use clap::Parser;
use ratatui::crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use ratatui::crossterm::execute;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::secrets::SecretStoreKind;

#[derive(Parser, Debug)]
#[command(
    name = "bandcamp-tui",
    version,
    about = "A terminal client for bandcamp.com"
)]
struct Cli {
    /// Where the Bandcamp session cookie is kept: the OS keyring (default) or a
    /// private file. The choice is remembered in the config file.
    #[arg(long, value_enum)]
    secret_store: Option<SecretStoreKind>,

    /// Log level for this application's own messages (RUST_LOG overrides it).
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Start without reading the stored session (the keyring is not touched);
    /// press L in the app to log in.
    #[arg(long)]
    no_login: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut config = Config::load().context("loading config")?;
    if let Some(kind) = cli.secret_store
        && kind != config.secret_store
    {
        config.secret_store = kind;
        config.save().context("saving config")?;
    }

    let _log_guard = init_logging(&cli.log_level)?;
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");
    redirect_stderr();

    let terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste);
    let result = app::run(terminal, config, cli.no_login).await;
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();

    if let Err(err) = &result {
        tracing::error!(?err, "exited with error");
    }
    result
}

/// Native libraries (ALSA in particular) print diagnostics to stderr, which would
/// scribble over the UI. Send stderr to a file next to the log instead.
fn redirect_stderr() {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let Ok(dir) = config::log_dir() else { return };
        let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("stderr.log"))
        else {
            return;
        };
        // SAFETY: dup2 on two valid descriptors; the file outlives the call and
        // fd 2 stays open, now pointing at the file.
        unsafe {
            libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
        }
    }
}

/// Log to a file in the state directory; the terminal is owned by the UI.
fn init_logging(level: &str) -> Result<WorkerGuard> {
    let dir = config::log_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = tracing_appender::rolling::never(&dir, "bandcamp-tui.log");
    let (writer, guard) = tracing_appender::non_blocking(file);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("warn,bandcamp_tui={level}")));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Ok(guard)
}
