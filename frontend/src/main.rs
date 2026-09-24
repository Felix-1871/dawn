// SPDX-License-Identifier: GPL-3.0-or-later

//! Entry point. All the actual wiring lives in `lib.rs` so the tests
//! can reuse it — see that file's doc comment.

use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use slint::ComponentHandle;

use frontend::backend::{BackendCommand, SocketBackend};
use frontend::data::DataFiles;
use frontend::mock_backend::{MockBackend, MockWifi};
use frontend::wifi::NetworkManager;
use frontend::{Services, build_ui, home_dir, load_config};

/// dawn: the LuminOS installer.
#[derive(Parser)]
#[command(name = "dawn")]
struct Cli {
    /// Use the real backend, but only report what an install would do:
    /// nothing runs as root and no disk is touched.
    #[arg(long, conflicts_with = "mock_backend")]
    dry_run: bool,

    /// Use canned answers instead of dawn-backend (SPEC.md's
    /// mock-backend mode), for working on the screens.
    #[arg(long)]
    mock_backend: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let (config, config_override) = match load_config() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let services = if cli.mock_backend {
        Services {
            backend: Arc::new(MockBackend::new().offline()),
            wifi: Arc::new(MockWifi),
            save_log_dir: home_dir(),
            data_files: DataFiles::system(),
        }
    } else {
        Services {
            backend: Arc::new(SocketBackend::new(BackendCommand::locate(
                cli.dry_run,
                config_override,
            ))),
            wifi: Arc::new(NetworkManager::new()),
            save_log_dir: home_dir(),
            data_files: DataFiles::system(),
        }
    };

    match build_ui(&config, services).and_then(|app| app.run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
