// SPDX-License-Identifier: GPL-3.0-or-later

//! M3's Done-when inside the e2e test VM: "Dawn installs a VM end to end
//! from the GUI, online and offline; a forced failure shows the error
//! screen with log." Runs the real `AppWindow`, the real socket client
//! and pkexec and backend, clicking through the screens with Slint's
//! testing backend (DECISIONS.md, M3); only pixel rendering is simulated,
//! so the VM needs no compositor. The test ISO's stand-in luminos-dawn
//! runs it at boot as the live user, and qemu-run.sh watches the serial
//! console for the markers printed here.
//!
//! Usage: gui_driver <online|offline|fail-then-offline|secure-boot> <target-disk> [--dry-run]
//!
//! `--dry-run` runs the installs as `dawn-backend --dry-run`, as the
//! current user without pkexec: the whole GUI flow over the real socket
//! protocol, on a development machine, touching nothing.
//!
//! - online: the repositories answer, so Network is skipped and the
//!   install pacstraps from the local mirror.
//! - offline: no network at all, so Network shows; Skip, then install
//!   from the live image.
//! - fail-then-offline: the mirror serves its databases but no packages,
//!   so pacstrap fails in step 5; the error screen must show the step,
//!   its log and the offline option, which then installs from the live
//!   image. qemu-run.sh boots this one from a USB stick with RAM to
//!   spare, so archiso has copied the image to RAM and unmounted the
//!   stick, and the install has to use the copy.
//!
//! - secure-boot: as online, on firmware in Setup Mode, keeping the
//!   Secure Boot checkbox, so the install sets up Secure Boot.
//!
//! In every scenario the Disk screen must offer the target disk and
//! nothing else: the live medium never shows.

#[path = "../tests/driver/mod.rs"]
mod driver;

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use driver::{Answers, Network, click, fill_to_summary, wait_for_outcome};
use frontend::backend::{Backend, BackendCommand, EventSink, Firmware, SocketBackend};
use frontend::data::DataFiles;
use frontend::wifi::NetworkManager;
use frontend::{AppWindow, Services, build_ui, home_dir, load_config};
use plan::InstallPlan;
use plan::protocol::{DiskInfo, Event};
use slint::Model as _;

/// An install from the local mirror or the image takes a few minutes; a
/// hang should still end the test long before CI's own timeout.
const INSTALL: Duration = Duration::from_secs(30 * 60);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(scenario), Some(target)) = (args.get(1).cloned(), args.get(2).cloned()) else {
        eprintln!(
            "usage: gui_driver <online|offline|fail-then-offline|secure-boot> <target-disk> [--dry-run]"
        );
        return ExitCode::from(2);
    };
    let dry_run = args.get(3).is_some_and(|arg| arg == "--dry-run");

    let outcome = driver::run(async move { drive(&scenario, &target, dry_run).await });
    match outcome {
        Ok(()) => {
            println!("DAWN-E2E-INSTALL-OK");
            ExitCode::SUCCESS
        }
        Err(err) => {
            println!("DAWN-E2E-INSTALL-FAILED: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn drive(scenario: &str, target: &str, dry_run: bool) -> Result<(), String> {
    let (config, config_override) = load_config()?;
    let services = Services {
        backend: Arc::new(EchoLog(SocketBackend::new(BackendCommand::locate(
            dry_run,
            config_override,
        )))),
        wifi: Arc::new(NetworkManager::new()),
        save_log_dir: home_dir(),
        data_files: DataFiles::system(),
    };
    let app = build_ui(&config, services).map_err(|err| err.to_string())?;
    let answers = Answers::fixture(target);
    // In the test VM, the target is the only disk Dawn may offer: the
    // live medium, a CD or a USB stick, never shows. A development
    // machine's own disks are none of the test's business.
    let only_target = !dry_run;

    match scenario {
        "online" => {
            fill_to_summary(&app, Network::Online, &answers).await?;
            if only_target {
                check_only_disk_offered(&app, target)?;
            }
            install_to_done(&app).await
        }
        "offline" => {
            fill_to_summary(&app, Network::Skip, &answers).await?;
            if only_target {
                check_only_disk_offered(&app, target)?;
            }
            install_to_done(&app).await
        }
        "secure-boot" => {
            let answers = Answers {
                secure_boot: true,
                ..answers
            };
            fill_to_summary(&app, Network::Online, &answers).await?;
            if !app.get_is_setup_mode() || !app.get_secure_boot_enroll() {
                return Err(
                    "the firmware should be in Setup Mode, with Secure Boot set up".to_string(),
                );
            }
            if only_target {
                check_only_disk_offered(&app, target)?;
            }
            install_to_done(&app).await
        }
        "fail-then-offline" => {
            if !dry_run {
                check_image_in_ram()?;
            }
            fill_to_summary(&app, Network::Online, &answers).await?;
            if only_target {
                check_only_disk_offered(&app, target)?;
            }
            click(&app, "Install")?;
            if wait_for_outcome(&app, INSTALL).await? != driver::ERROR {
                return Err("the install should have failed at pacstrap, but finished".to_string());
            }
            println!("error screen:\n{}", driver::describe_error(&app));
            if app.get_error_step() != 5 || !app.get_error_offline_fallback() {
                return Err("expected a step 5 failure offering the offline install".to_string());
            }
            if !app.get_error_log().contains("failed retrieving file") {
                return Err("the error screen's log doesn't show pacstrap's failure".to_string());
            }
            println!("DAWN-E2E-ERROR-SCREEN-OK");
            click(&app, "Install offline instead")?;
            wait_until_done(&app).await
        }
        other => Err(format!("unknown scenario {other:?}")),
    }
}

/// The real backend, with every install log line also printed here: in
/// the test VM that's the serial console, so a failed run's log ends up
/// in CI's. The backend has already redacted the lines.
struct EchoLog(SocketBackend);

impl Backend for EchoLog {
    fn list_disks(&self) -> Result<Vec<DiskInfo>, String> {
        self.0.list_disks()
    }

    fn probe_firmware(&self) -> Result<Firmware, String> {
        self.0.probe_firmware()
    }

    fn check_online(&self) -> Result<bool, String> {
        self.0.check_online()
    }

    fn validate(&self, plan: &InstallPlan) -> Result<Vec<String>, String> {
        self.0.validate(plan)
    }

    fn install(&self, plan: &InstallPlan, on_event: EventSink) -> Result<(), String> {
        self.0.install(
            plan,
            Arc::new(move |event| {
                if let Event::Log { line } = &event {
                    println!("dawn: {line}");
                }
                on_event(event);
            }),
        )
    }
}

fn check_only_disk_offered(app: &AppWindow, target: &str) -> Result<(), String> {
    let offered: Vec<String> = app
        .get_disks()
        .iter()
        .map(|disk| disk.device.to_string())
        .collect();
    if offered == [target] {
        Ok(())
    } else {
        Err(format!(
            "the Disk screen should offer only {target}, but lists {offered:?}"
        ))
    }
}

/// archiso copied the live image to RAM and unmounted the medium it
/// booted from, so an offline install can only work from the copy.
fn check_image_in_ram() -> Result<(), String> {
    if !Path::new("/run/archiso/copytoram/airootfs.sfs").exists() {
        return Err("archiso didn't copy the live image to RAM".to_string());
    }
    if Path::new("/run/archiso/bootmnt").exists() {
        return Err("archiso copied the live image to RAM but kept the medium mounted".to_string());
    }
    println!("copytoram: the live image is in RAM and the boot medium is unmounted");
    Ok(())
}

async fn install_to_done(app: &AppWindow) -> Result<(), String> {
    click(app, "Install")?;
    wait_until_done(app).await
}

async fn wait_until_done(app: &AppWindow) -> Result<(), String> {
    if wait_for_outcome(app, INSTALL).await? == driver::DONE {
        Ok(())
    } else {
        Err(format!(
            "the install failed: {}",
            driver::describe_error(app)
        ))
    }
}
