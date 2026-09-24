// SPDX-License-Identifier: GPL-3.0-or-later

//! Drives the real `AppWindow` through Slint's testing backend: clicks
//! by accessible label (SPEC.md "UI smoke tests use Slint's testing
//! backend to find elements by accessible label and drive Next and
//! Back"), and waits for the backend's answers the way a person would,
//! with the event loop running. Shared by `tests/clickthrough.rs` (mock
//! backend) and `examples/gui_driver.rs` (the real backend, inside the
//! e2e test VM).
//!
//! ComboBox-backed fields (locale, keyboard layout, timezone) are set
//! through their properties: std-widgets' ComboBox has no accessible
//! set-value action (DECISIONS.md, M2).

// Each includer uses a different part of this.
#![allow(dead_code)]

use std::future::Future;
use std::time::{Duration, Instant};

use frontend::{AppWindow, Theme};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle as _, Model};

pub const WELCOME: i32 = 0;
pub const NETWORK: i32 = 1;
pub const KEYBOARD: i32 = 2;
pub const TIMEZONE: i32 = 3;
pub const DISK: i32 = 4;
pub const ACCOUNT: i32 = 5;
pub const SUMMARY: i32 = 6;
pub const INSTALLING: i32 = 7;
pub const DONE: i32 = 8;
pub const ERROR: i32 = 9;

/// Runs `scenario` inside Slint's event loop, as the GUI's own work
/// happens there, and returns what it returned.
pub fn run<F>(scenario: F) -> Result<(), String>
where
    F: Future<Output = Result<(), String>> + 'static,
{
    i_slint_backend_testing::init_integration_test_with_system_time();
    let outcome = std::rc::Rc::new(std::cell::RefCell::new(None));
    let result = outcome.clone();
    slint::spawn_local(async move {
        *result.borrow_mut() = Some(scenario.await);
        let _ = slint::quit_event_loop();
    })
    .map_err(|err| err.to_string())?;
    slint::run_event_loop().map_err(|err| err.to_string())?;
    let result = outcome.borrow_mut().take();
    result.unwrap_or_else(|| Err("the scenario never finished".to_string()))
}

pub async fn sleep(duration: Duration) {
    let (done, finished) = futures_channel::oneshot::channel();
    slint::Timer::single_shot(duration, move || {
        let _ = done.send(());
    });
    let _ = finished.await;
}

pub async fn wait_until(
    what: &str,
    timeout: Duration,
    mut condition: impl FnMut() -> bool,
) -> Result<(), String> {
    let started = Instant::now();
    while !condition() {
        if started.elapsed() > timeout {
            return Err(format!("timed out after {timeout:?} waiting for {what}"));
        }
        sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

pub async fn wait_for_screen(
    app: &AppWindow,
    screen: i32,
    timeout: Duration,
) -> Result<(), String> {
    wait_until(&format!("screen {screen}"), timeout, || {
        app.get_current_screen() == screen
    })
    .await
    .map_err(|err| format!("{err} (on screen {})", app.get_current_screen()))
}

/// Invokes the default action of the one element labelled `label`,
/// the way a screen reader clicks it.
pub fn click(app: &AppWindow, label: &str) -> Result<(), String> {
    let screen = app.get_current_screen();
    let mut matches = ElementHandle::find_by_accessible_label(app, label);
    let element = matches
        .next()
        .ok_or_else(|| format!("nothing labelled {label:?} on screen {screen}"))?;
    if matches.next().is_some() {
        return Err(format!(
            "more than one element labelled {label:?} on screen {screen}"
        ));
    }
    element.invoke_accessible_default_action();
    Ok(())
}

pub fn expect_screen(app: &AppWindow, screen: i32) -> Result<(), String> {
    let current = app.get_current_screen();
    if current == screen {
        Ok(())
    } else {
        Err(format!("expected screen {screen}, on screen {current}"))
    }
}

/// How the Network screen is handled when it shows.
pub enum Network {
    /// It mustn't show: the PC is online.
    Online,
    /// Skip it, for an offline install.
    Skip,
    /// Join this network, with this password.
    Join(&'static str, &'static str),
}

/// The answers tests/plans/erase-online.json was written from.
pub struct Answers {
    pub timezone: &'static str,
    pub full_name: &'static str,
    pub username: &'static str,
    pub hostname: &'static str,
    pub password: &'static str,
    /// The `/dev/disk/by-id/` path of the disk to pick.
    pub disk: String,
    /// The Disk screen's Secure Boot checkbox, where it shows (only in
    /// Setup Mode, as a fresh VM's firmware is).
    pub secure_boot: bool,
}

impl Answers {
    pub fn fixture(disk: impl Into<String>) -> Self {
        Self {
            timezone: "Europe/Berlin",
            full_name: "Ada Lovelace",
            username: "ada",
            hostname: "ada-laptop",
            password: "correct-horse-battery-staple",
            disk: disk.into(),
            secure_boot: false,
        }
    }
}

/// From the Welcome screen to the Summary screen, answering every screen
/// on the way, the way a person would.
pub async fn fill_to_summary(
    app: &AppWindow,
    network: Network,
    answers: &Answers,
) -> Result<(), String> {
    expect_screen(app, WELCOME)?;
    wait_until("the backend's probes", Duration::from_secs(120), || {
        !app.get_probing()
    })
    .await?;
    let probe_error = app.get_probe_error();
    if !probe_error.is_empty() {
        return Err(format!("the probes failed: {probe_error}"));
    }
    click(app, "Install")?;

    match network {
        Network::Online => {
            if app.get_is_offline() {
                return Err("the PC should be online, but the Network screen showed".to_string());
            }
        }
        Network::Skip => {
            expect_screen(app, NETWORK)?;
            click(app, "Skip")?;
        }
        Network::Join(ssid, password) => {
            expect_screen(app, NETWORK)?;
            wait_until("the Wi-Fi scan", Duration::from_secs(30), || {
                !app.get_wifi_busy()
            })
            .await?;
            click(app, &format!("Wi-Fi network {ssid}"))?;
            app.set_wifi_password(password.into());
            click(app, "Join")?;
            wait_for_screen(app, KEYBOARD, Duration::from_secs(60)).await?;
        }
    }

    expect_screen(app, KEYBOARD)?;
    app.set_kb_layout("us".into());
    app.set_kb_variant("".into());
    click(app, "Next")?;

    expect_screen(app, TIMEZONE)?;
    app.set_timezone(answers.timezone.into());
    click(app, "Next")?;

    expect_screen(app, DISK)?;
    let disks = app.get_disks();
    let disk = disks
        .iter()
        .find(|disk| disk.device == answers.disk.as_str())
        .ok_or_else(|| {
            let offered: Vec<String> = disks.iter().map(|d| d.device.to_string()).collect();
            format!(
                "{} isn't offered; the Disk screen lists {offered:?}",
                answers.disk
            )
        })?;
    click(app, &format!("{} {}", disk.model, disk.serial))?;
    if app.get_selected_device() != answers.disk.as_str() {
        return Err(format!(
            "picking {} selected {}",
            answers.disk,
            app.get_selected_device()
        ));
    }
    // Checked by default in Setup Mode (SPEC.md), so answering "no" means
    // unticking it.
    if app.get_is_setup_mode() && app.get_secure_boot_enroll() != answers.secure_boot {
        let product = app.global::<Theme>().get_product_name();
        click(app, &format!("Set up Secure Boot with {product} keys"))?;
        if app.get_secure_boot_enroll() != answers.secure_boot {
            return Err("the Secure Boot checkbox didn't change".to_string());
        }
    }
    click(app, "Next")?;

    // The full name drives the real derivation, through the callback a
    // LineEdit's `edited` fires; the hostname is then edited by hand, as
    // SPEC.md says both fields allow.
    expect_screen(app, ACCOUNT)?;
    app.set_full_name(answers.full_name.into());
    app.invoke_full_name_edited(answers.full_name.into());
    if app.get_username() != answers.username {
        return Err(format!(
            "{:?} derived username {:?}",
            answers.full_name,
            app.get_username()
        ));
    }
    app.set_hostname(answers.hostname.into());
    app.invoke_hostname_edited(answers.hostname.into());
    app.set_password(answers.password.into());
    click(app, "Next")?;

    expect_screen(app, SUMMARY)
}

/// The install's outcome screen, Done or Error, whichever comes.
pub async fn wait_for_outcome(app: &AppWindow, timeout: Duration) -> Result<i32, String> {
    wait_until("the install to finish", timeout, || {
        matches!(app.get_current_screen(), DONE | ERROR)
    })
    .await?;
    Ok(app.get_current_screen())
}

/// What the error screen shows, for a failure message.
pub fn describe_error(app: &AppWindow) -> String {
    format!(
        "step {}: {}\n{}",
        app.get_error_step(),
        app.get_error_message(),
        app.get_error_log()
    )
}
