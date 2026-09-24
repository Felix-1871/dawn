// SPDX-License-Identifier: GPL-3.0-or-later

//! Drives the real `AppWindow` through Slint's testing backend: clicks
//! by accessible label (SPEC.md "UI smoke tests use Slint's testing
//! backend to find elements by accessible label and drive Next and
//! Back"), and waits for the backend's answers the way a person would,
//! with the event loop running. Shared by `tests/clickthrough.rs` (mock
//! backend) and `examples/gui_driver.rs` (the real backend, inside the
//! e2e test VM).
//!
//! Search fields are typed into through their accessible set-value
//! action, which is how a screen reader types, and fires the same
//! `edited` a keyboard does.

// Each includer uses a different part of this.
#![allow(dead_code)]

use std::future::Future;
use std::time::{Duration, Instant};

use frontend::data::{DataFiles, keyboard_value};
use frontend::{AppWindow, Choice, Theme};
use i_slint_backend_testing::ElementHandle;
use slint::{ComponentHandle as _, Model, ModelRc};

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

/// Types `text` into the one field labelled `label`, the way a screen
/// reader sets a value.
pub fn type_into(app: &AppWindow, label: &str, text: &str) -> Result<(), String> {
    let mut matches = ElementHandle::find_by_accessible_label(app, label);
    let field = matches
        .next()
        .ok_or_else(|| format!("no field labelled {label:?}"))?;
    if matches.next().is_some() {
        return Err(format!("more than one field labelled {label:?}"));
    }
    field.set_accessible_value(text);
    Ok(())
}

/// Searches a list for `value`, what the entry puts into the plan, and
/// clicks that entry by whatever the list calls it. An exact match is
/// the first a search shows, so its row is one the list has made; a
/// list only makes the rows it shows.
pub fn pick(
    app: &AppWindow,
    search: &str,
    value: &str,
    choices: impl Fn(&AppWindow) -> ModelRc<Choice>,
) -> Result<(), String> {
    type_into(app, search, value)?;
    let label = choices(app)
        .iter()
        .find(|choice| choice.value == value)
        .map(|choice| choice.label.to_string())
        .ok_or_else(|| {
            let found: Vec<String> = choices(app).iter().map(|c| c.value.to_string()).collect();
            format!("searching for {value:?} found {found:?}")
        })?;
    click(app, &label)
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
    pub locale: &'static str,
    /// As XKB writes it: `us`, or `de(nodeadkeys)` for a variant.
    pub keyboard: &'static str,
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
            locale: "en_US.UTF-8",
            keyboard: "us",
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
    pick(app, "Search languages", answers.locale, |app| {
        app.get_locale_choices()
    })?;
    if app.get_locale() != answers.locale {
        return Err(format!(
            "picking {} chose {}",
            answers.locale,
            app.get_locale()
        ));
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
    pick(app, "Search keyboard layouts", answers.keyboard, |app| {
        app.get_keyboard_choices()
    })?;
    let picked = keyboard_value(&app.get_kb_layout(), &app.get_kb_variant());
    if picked != answers.keyboard {
        return Err(format!("picking {} chose {picked}", answers.keyboard));
    }
    click(app, "Next")?;

    expect_screen(app, TIMEZONE)?;
    pick(
        app,
        "Search cities, regions or countries",
        answers.timezone,
        |app| app.get_timezone_choices(),
    )?;
    if app.get_timezone() != answers.timezone {
        return Err(format!(
            "picking {} chose {}",
            answers.timezone,
            app.get_timezone()
        ));
    }
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

/// Small copies of the system files the language, keyboard and timezone
/// lists come from, so the UI tests don't depend on the machine's own.
pub fn fixture_data_files() -> DataFiles {
    let dir = std::env::temp_dir().join(format!("dawn-ui-data-{}", std::process::id()));
    let files: &[(&str, &str)] = &[
        (
            "SUPPORTED",
            "de_DE.UTF-8 UTF-8\nde_DE ISO-8859-1\nen_US.UTF-8 UTF-8\n",
        ),
        (
            "locales/de_DE",
            "language \"German\"\nterritory \"Germany\"\n",
        ),
        // glibc's own name for it.
        (
            "locales/en_US",
            "language \"American English\"\nterritory \"United States\"\n",
        ),
        (
            "evdev.lst",
            "! layout\n  us  English (US)\n  de  German\n  gb  English (UK)\n\
             ! variant\n  nodeadkeys  de: German (no dead keys)\n  intl  us: English (US, intl., with dead keys)\n",
        ),
        (
            "kbd-model-map",
            "de\tde\tpc105\t-\tterminate:ctrl_alt_bksp\tde-DE,de\n\
             us\tus\tpc105+inet\t-\tterminate:ctrl_alt_bksp\ten-US,en\n",
        ),
        (
            "zone.tab",
            "DE\t+5230+01322\tEurope/Berlin\tmost of Germany\n\
             US\t+404251-0740023\tAmerica/New_York\tEastern (most areas)\n",
        ),
        ("iso3166.tab", "DE\tGermany\nUS\tUnited States\n"),
    ];
    for (name, content) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().expect("a parent directory")).expect("a temp dir");
        std::fs::write(&path, content).expect("a temp file");
    }
    DataFiles {
        supported_locales: dir.join("SUPPORTED"),
        locale_sources: dir.join("locales"),
        xkb_rules: dir.join("evdev.lst"),
        kbd_model_map: dir.join("kbd-model-map"),
        zone_tab: dir.join("zone.tab"),
        iso3166_tab: dir.join("iso3166.tab"),
    }
}
