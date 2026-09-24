// SPDX-License-Identifier: GPL-3.0-or-later

//! The UI smoke test (SPEC.md "Testing": "Every screen opens, Next and
//! Back work, Network appears only when offline | Slint testing backend
//! against the mock backend"). Drives the same `AppWindow` the `dawn`
//! binary runs, through `build_ui()`, with the mock backend standing in
//! for `dawn-backend`.
//!
//! Still M2's Done-when check ("Clicking through produces a valid plan
//! identical to a hand-written one"), plus M3's flows (the Network screen
//! when offline, the error screen after a failed install, Save log,
//! Retry from start and the offline fallback) and M4's: the Secure Boot
//! checkbox in Setup Mode, and the language, keyboard and timezone
//! lists, read from small copies of the system files they come from.
//!
//! Slint's testing backend is set up once per process, so every scenario
//! runs in sequence inside one test.

mod driver;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use driver::{Answers, Network, click, expect_screen, fill_to_summary, wait_for_outcome};
use frontend::live_session::{LiveKeyboard, NoLiveKeyboard};
use frontend::mock_backend::{self, MockBackend, MockWifi};
use frontend::{AppWindow, Services, build_ui};
use plan::Source;
use plan::config::InstallerConfig;
use slint::Model as _;

fn fixture() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/plans/erase-online.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn log_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dawn-clickthrough-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn start(backend: &Arc<MockBackend>) -> Result<AppWindow, String> {
    start_with(backend, Arc::new(NoLiveKeyboard))
}

fn start_with(
    backend: &Arc<MockBackend>,
    live_keyboard: Arc<dyn LiveKeyboard>,
) -> Result<AppWindow, String> {
    let config = InstallerConfig::builtin_default().map_err(|err| err.to_string())?;
    build_ui(
        &config,
        Services {
            backend: backend.clone(),
            wifi: Arc::new(MockWifi),
            save_log_dir: log_dir(),
            data_files: driver::fixture_data_files(),
            live_keyboard,
        },
    )
    .map_err(|err| err.to_string())
}

/// Stands in for the live Hyprland session: records every switch, and
/// can refuse them.
#[derive(Default)]
struct RecordingKeyboard {
    switches: Mutex<Vec<(String, String)>>,
    refuse: bool,
}

impl RecordingKeyboard {
    fn last(&self) -> Option<(String, String)> {
        self.switches.lock().ok()?.last().cloned()
    }
}

impl LiveKeyboard for RecordingKeyboard {
    fn switch(&self, layout: &str, variant: &str) -> Result<(), String> {
        if let Ok(mut switches) = self.switches.lock() {
            switches.push((layout.to_string(), variant.to_string()));
        }
        if self.refuse {
            Err("no session to switch".to_string())
        } else {
            Ok(())
        }
    }
}

const OUTCOME: Duration = Duration::from_secs(10);

/// M2's Done-when: the plan the backend is asked to install is the
/// hand-written fixture, exactly.
async fn clicking_through_online_installs_the_fixture_plan() -> Result<(), String> {
    let backend = Arc::new(MockBackend::new());
    let app = start(&backend)?;
    fill_to_summary(&app, Network::Online, &Answers::fixture(mock_backend::DISK)).await?;
    click(&app, "Install")?;
    let outcome = wait_for_outcome(&app, OUTCOME).await?;
    expect_screen(&app, driver::DONE)
        .map_err(|err| format!("{err}: {}", driver::describe_error(&app)))?;
    assert_eq!(outcome, driver::DONE);

    let installs = backend.installs();
    assert_eq!(installs.len(), 1, "exactly one install should have started");
    let produced = serde_json::to_value(&installs[0]).map_err(|err| err.to_string())?;
    assert_eq!(
        produced,
        fixture(),
        "the clicked-through plan should be identical to tests/plans/erase-online.json"
    );
    Ok(())
}

async fn back_from_keyboard_skips_network_when_online() -> Result<(), String> {
    let backend = Arc::new(MockBackend::new());
    let app = start(&backend)?;
    driver::wait_until("the probes", OUTCOME, || !app.get_probing()).await?;
    click(&app, "Install")?;
    expect_screen(&app, driver::KEYBOARD)?;
    click(&app, "Back")?;
    expect_screen(&app, driver::WELCOME)
}

async fn skipping_wifi_installs_from_the_live_image() -> Result<(), String> {
    let backend = Arc::new(MockBackend::new().offline());
    let app = start(&backend)?;
    fill_to_summary(&app, Network::Skip, &Answers::fixture(mock_backend::DISK)).await?;
    click(&app, "Install")?;
    wait_for_outcome(&app, OUTCOME).await?;
    expect_screen(&app, driver::DONE)?;
    let plan = &backend.installs()[0];
    assert_eq!(plan.source, Source::Squashfs);
    assert_eq!(plan.network_profile, None);
    Ok(())
}

async fn joining_wifi_carries_the_profile_into_the_plan() -> Result<(), String> {
    let backend = Arc::new(MockBackend::new().offline());
    let app = start(&backend)?;
    fill_to_summary(
        &app,
        Network::Join("home-wifi", "hunter22"),
        &Answers::fixture(mock_backend::DISK),
    )
    .await?;
    click(&app, "Install")?;
    wait_for_outcome(&app, OUTCOME).await?;
    let plan = &backend.installs()[0];
    assert_eq!(plan.network_profile.as_deref(), Some("home-wifi"));
    // The mock's repositories stay unreachable after joining, so the
    // install is still offline, with the network set up regardless.
    assert_eq!(plan.source, Source::Squashfs);
    Ok(())
}

/// M3's Done-when, from the GUI's side: a failed install shows the error
/// screen with its log, the log can be saved, Retry from start runs the
/// same plan again, and a network failure offers the offline install.
async fn a_failed_install_shows_the_error_screen_with_its_log() -> Result<(), String> {
    let backend = Arc::new(MockBackend::new().failing_online_installs());
    let app = start(&backend)?;
    fill_to_summary(&app, Network::Online, &Answers::fixture(mock_backend::DISK)).await?;
    click(&app, "Install")?;
    assert_eq!(wait_for_outcome(&app, OUTCOME).await?, driver::ERROR);
    assert_eq!(app.get_error_step(), 5);
    assert!(app.get_error_offline_fallback());
    assert!(app.get_error_log().contains("failed retrieving file"));

    click(&app, "Save log")?;
    let saved = app.get_saved_log_path().to_string();
    let written = std::fs::read_to_string(&saved).map_err(|err| format!("{saved}: {err}"))?;
    assert!(written.contains("==> Step 5: Install the base system"));
    assert!(!written.contains("correct-horse-battery-staple"));

    click(&app, "Retry from start")?;
    assert_eq!(wait_for_outcome(&app, OUTCOME).await?, driver::ERROR);
    assert_eq!(backend.installs().len(), 2);
    assert_eq!(backend.installs()[1].source, Source::Pacstrap);

    click(&app, "Install offline instead")?;
    assert_eq!(wait_for_outcome(&app, OUTCOME).await?, driver::DONE);
    let installs = backend.installs();
    assert_eq!(installs.len(), 3);
    assert_eq!(installs[2].source, Source::Squashfs);
    std::fs::remove_file(saved).ok();
    Ok(())
}

/// SPEC.md: "Secure Boot checkbox appears only in Setup Mode, checked by
/// default." Keeping it asks the backend to set up Secure Boot; unticking
/// it leaves Secure Boot alone.
async fn secure_boot_is_offered_in_setup_mode() -> Result<(), String> {
    for keep in [true, false] {
        let backend = Arc::new(MockBackend::new().in_setup_mode());
        let app = start(&backend)?;
        driver::wait_until("the probes", OUTCOME, || !app.get_probing()).await?;
        assert!(
            app.get_secure_boot_enroll(),
            "checked by default in Setup Mode"
        );
        let mut answers = Answers::fixture(mock_backend::DISK);
        answers.secure_boot = keep;
        fill_to_summary(&app, Network::Online, &answers).await?;
        click(&app, "Install")?;
        wait_for_outcome(&app, OUTCOME).await?;
        assert_eq!(backend.installs()[0].secure_boot.enroll, keep);
    }
    Ok(())
}

/// SPEC.md: the timezone is "guessed from the chosen locale", and so is
/// the keyboard (DECISIONS.md, M4). What the user picked by hand stays
/// when the language changes again.
async fn a_language_preselects_its_keyboard_and_timezone() -> Result<(), String> {
    let backend = Arc::new(MockBackend::new());
    let app = start(&backend)?;
    driver::wait_until("the lists", OUTCOME, || {
        app.get_locale_choices().row_count() > 0
    })
    .await?;
    assert_eq!(app.get_locale_label(), "American English (United States)");
    assert_eq!(app.get_kb_layout(), "us");
    assert_eq!(app.get_timezone(), "America/New_York");

    driver::pick(&app, "Search languages", "de_DE.UTF-8", |app| {
        app.get_locale_choices()
    })?;
    assert_eq!(app.get_locale(), "de_DE.UTF-8");
    assert_eq!(app.get_kb_layout(), "de");
    assert_eq!(app.get_keyboard_label(), "German");
    assert_eq!(app.get_timezone(), "Europe/Berlin");
    assert_eq!(app.get_timezone_label(), "Europe/Berlin");

    driver::wait_until("the probes", OUTCOME, || !app.get_probing()).await?;
    click(&app, "Install")?;
    expect_screen(&app, driver::KEYBOARD)?;
    driver::pick(&app, "Search keyboard layouts", "de(nodeadkeys)", |app| {
        app.get_keyboard_choices()
    })?;
    assert_eq!(app.get_kb_variant(), "nodeadkeys");
    click(&app, "Back")?;
    expect_screen(&app, driver::WELCOME)?;

    driver::pick(&app, "Search languages", "en_US.UTF-8", |app| {
        app.get_locale_choices()
    })?;
    assert_eq!(
        (app.get_kb_layout().as_str(), app.get_kb_variant().as_str()),
        ("de", "nodeadkeys"),
        "a keyboard picked by hand stays"
    );
    assert_eq!(app.get_timezone(), "America/New_York");
    Ok(())
}

/// The Keyboard screen switches the live session to the layout shown,
/// and to each one picked (decided with the user), so the preview field
/// and the password type in it.
async fn the_keyboard_screen_switches_the_live_session() -> Result<(), String> {
    let keyboard = Arc::new(RecordingKeyboard::default());
    let app = start_with(&Arc::new(MockBackend::new()), keyboard.clone())?;
    driver::wait_until("the probes", OUTCOME, || !app.get_probing()).await?;
    assert_eq!(
        keyboard.last(),
        None,
        "picking a language switches nothing yet"
    );
    click(&app, "Install")?;
    expect_screen(&app, driver::KEYBOARD)?;
    driver::wait_until("the switch to the layout shown", OUTCOME, || {
        keyboard.last() == Some(("us".to_string(), String::new()))
    })
    .await?;
    driver::pick(&app, "Search keyboard layouts", "de(nodeadkeys)", |app| {
        app.get_keyboard_choices()
    })?;
    driver::wait_until("the switch to the layout picked", OUTCOME, || {
        keyboard.last() == Some(("de".to_string(), "nodeadkeys".to_string()))
    })
    .await?;
    assert_eq!(app.get_keyboard_switch_error(), "");

    // A session that can't be switched says so on the screen.
    let refusing = Arc::new(RecordingKeyboard {
        refuse: true,
        ..RecordingKeyboard::default()
    });
    let app = start_with(&Arc::new(MockBackend::new()), refusing)?;
    driver::wait_until("the probes", OUTCOME, || !app.get_probing()).await?;
    click(&app, "Install")?;
    driver::wait_until("the error", OUTCOME, || {
        !app.get_keyboard_switch_error().is_empty()
    })
    .await
}

async fn an_unavailable_disk_cant_be_picked() -> Result<(), String> {
    let backend = Arc::new(MockBackend::new());
    let app = start(&backend)?;
    let answers = Answers::fixture("/dev/disk/by-id/usb-EXAMPLE_STICK-0:0");
    let error = fill_to_summary(&app, Network::Online, &answers)
        .await
        .unwrap_err();
    assert!(error.contains("selected"), "{error}");
    expect_screen(&app, driver::DISK)
}

#[test]
fn clickthrough() {
    driver::run(async {
        clicking_through_online_installs_the_fixture_plan().await?;
        back_from_keyboard_skips_network_when_online().await?;
        skipping_wifi_installs_from_the_live_image().await?;
        joining_wifi_carries_the_profile_into_the_plan().await?;
        a_failed_install_shows_the_error_screen_with_its_log().await?;
        secure_boot_is_offered_in_setup_mode().await?;
        a_language_preselects_its_keyboard_and_timezone().await?;
        the_keyboard_screen_switches_the_live_session().await?;
        an_unavailable_disk_cant_be_picked().await?;
        Ok(())
    })
    .unwrap();
}
