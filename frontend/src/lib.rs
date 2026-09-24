// SPDX-License-Identifier: GPL-3.0-or-later

//! `dawn`: the Slint GUI. Reads `installer.toml` and branding, asks the
//! backend about this PC, drives the screens, builds the `InstallPlan`
//! and shows the install's progress — see SPEC.md "Architecture".
//!
//! Split into a library so the UI smoke test (`tests/clickthrough.rs`)
//! and the in-VM end-to-end driver (`examples/gui_driver.rs`) drive the
//! same `AppWindow` the real binary (`src/main.rs`) runs.
//!
//! Anything slow (the backend's probes, Wi-Fi, the install itself) runs
//! on another thread and reports back through
//! `upgrade_in_event_loop`, so the window never stalls.

pub mod backend;
pub mod branding;
pub mod data;
pub mod mock_backend;
pub mod state;
pub mod system;
pub mod wifi;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use plan::config::{self, InstallerConfig};
use plan::protocol::{self, Event};
use plan::{InstallPlan, Source};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use backend::{Backend, EventSink};
use data::{Data, DataFiles};
use wifi::{ConnectError, Wifi};

slint::include_modules!();

/// The stable identifier SPEC.md's Hyprland window rule matches on
/// ("a stable window title and app_id"). Slint's winit backend doesn't
/// set one on its own (confirmed by inspecting a running window with
/// `hyprctl clients` — its `class` came back empty) — see DECISIONS.md.
pub const APP_ID: &str = "luminos-dawn";

/// How many log lines the Installing screen keeps; Save log writes them
/// all, and the backend keeps the whole log in /var/log/dawn.log.
const SHOWN_LOG_LINES: usize = 500;

/// What the GUI talks to.
#[derive(Clone)]
pub struct Services {
    pub backend: Arc<dyn Backend>,
    pub wifi: Arc<dyn Wifi>,
    /// Where the error screen's Save log writes `dawn-install.log`.
    pub save_log_dir: PathBuf,
    /// Where the language, keyboard and timezone lists come from.
    pub data_files: DataFiles,
}

/// The live user's home directory, where Save log puts the log.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// `DAWN_CONFIG` if set, else `/etc/dawn/installer.toml`, else the
/// compiled-in copy of `config/installer.toml`. Also returns the path
/// when it came from `DAWN_CONFIG`, for passing on to backends that run
/// as the user.
pub fn load_config() -> Result<(InstallerConfig, Option<PathBuf>), String> {
    let (path, overridden) = match std::env::var_os("DAWN_CONFIG") {
        Some(path) => (PathBuf::from(path), true),
        None => (PathBuf::from(config::SYSTEM_PATH), false),
    };
    if !overridden && !path.exists() {
        return InstallerConfig::builtin_default()
            .map(|config| (config, None))
            .map_err(|err| err.to_string());
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?;
    let config =
        InstallerConfig::from_toml_str(&raw).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok((config, overridden.then_some(path)))
}

/// The install in progress, shared between the UI thread and the thread
/// delivering the backend's events.
#[derive(Default)]
struct Session {
    /// The last plan sent, for "Retry from start" and "Install offline
    /// instead".
    plan: Option<InstallPlan>,
    /// Every log line of the current install, for Save log.
    log: Vec<String>,
}

/// Builds an `AppWindow` with branding loaded and every callback wired,
/// and starts the backend's probes. Everything `main()` needs before
/// calling `.run()`, and everything the tests need before driving it.
pub fn build_ui(
    config: &InstallerConfig,
    services: Services,
) -> Result<AppWindow, slint::PlatformError> {
    let app = AppWindow::new()?;
    // Errors here just mean no Wayland/X11 platform is present (the
    // testing backend, say) — harmless to ignore per set_xdg_app_id's
    // own doc comment.
    let _ = slint::set_xdg_app_id(APP_ID);

    let branding_dir = branding::find_branding_dir(&config.branding);
    match branding::load(&branding_dir) {
        Ok(branding) => apply_branding(&app, branding),
        Err(err) => eprintln!(
            "warning: could not load branding from {}: {err}",
            branding_dir.display()
        ),
    }

    app.set_autologin_option(config.screens.autologin_option);
    app.set_install_log(ModelRc::new(VecModel::<SharedString>::default()));

    let session = Arc::new(Mutex::new(Session::default()));
    let lists = Arc::new(Mutex::new(Lists::default()));
    wire_lists(&app, &lists);
    wire_account_derivation(&app);
    wire_wifi(&app, &services);
    wire_install(&app, &services, &session, config.source.packages.clone());
    wire_error_screen(&app, &services, &session);
    wire_done_screen(&app);
    start_probes(&app, &services, &lists);

    Ok(app)
}

pub fn apply_branding(app: &AppWindow, branding: branding::Branding) {
    let theme = app.global::<Theme>();
    theme.set_product_name(branding.product_name.into());
    theme.set_version_string(branding.version_string.into());
    theme.set_accent(branding.accent_color);
    theme.set_website(branding.website.into());
    theme.set_support_url(branding.support_url.into());
    theme.set_secure_boot_url(branding.secure_boot_url.into());
    theme.set_logo(branding.logo.clone());
    theme.set_icon(branding.icon);
    theme.set_welcome_text(branding.welcome_text.into());
}

/// Firmware, disks, whether the repositories are reachable, and whether
/// there's Wi-Fi: what the Welcome screen waits for before it lets the
/// install start.
fn start_probes(app: &AppWindow, services: &Services, lists: &Arc<Mutex<Lists>>) {
    let weak = app.as_weak();
    let services = services.clone();
    let lists = Arc::clone(lists);
    std::thread::spawn(move || {
        // The lists first: they're quick, and the Welcome screen shows
        // one while the backend's probes run.
        let data = Data::load(&services.data_files);
        if let Ok(mut lists) = lists.lock() {
            lists.data = data;
        }
        let shown = Arc::clone(&lists);
        let _ = weak.upgrade_in_event_loop(move |app| show_lists(&app, &shown));

        let firmware = services.backend.probe_firmware();
        let disks = services.backend.list_disks();
        let online = services.backend.check_online();
        let wifi_available = services.wifi.available();
        let _ = weak.upgrade_in_event_loop(move |app| {
            let mut problems = Vec::new();
            match firmware {
                Ok(firmware) => {
                    app.set_is_uefi(firmware.uefi);
                    app.set_is_setup_mode(firmware.setup_mode);
                    app.set_firmware_secure_boot(firmware.secure_boot);
                    // SPEC.md: "Secure Boot checkbox appears only in Setup
                    // Mode, checked by default."
                    app.set_secure_boot_enroll(firmware.setup_mode);
                }
                Err(err) => problems.push(err),
            }
            match disks {
                Ok(disks) => app.set_disks(disk_model(disks)),
                Err(err) => problems.push(err),
            }
            let offline = match online {
                Ok(online) => !online,
                Err(err) => {
                    problems.push(err);
                    true
                }
            };
            app.set_is_offline(offline);
            app.set_wifi_available(wifi_available);
            app.set_probe_error(problems.join("; ").into());
            app.set_probing(false);
            if offline && wifi_available {
                scan_wifi(&app, &services);
            }
        });
    });
}

/// The language, keyboard and timezone lists (empty until loaded), and
/// which of the keyboard and timezone the user picked by hand, so a new
/// language's guesses don't overwrite them.
#[derive(Default)]
struct Lists {
    data: Data,
    keyboard_picked: bool,
    timezone_picked: bool,
}

fn choice_model(choices: Vec<data::Choice>) -> ModelRc<Choice> {
    let rows: Vec<Choice> = choices
        .into_iter()
        .map(|choice| Choice {
            value: choice.value.into(),
            label: choice.label.into(),
            detail: choice.detail.into(),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// Fills the three lists, narrowed to their search fields, once loaded,
/// and pre-selects the keyboard and timezone for the current language.
fn show_lists(app: &AppWindow, lists: &Arc<Mutex<Lists>>) {
    let Ok(mut lists) = lists.lock() else {
        return;
    };
    app.set_locale_choices(choice_model(data::search(
        &lists.data.locales,
        &app.get_locale_query(),
    )));
    app.set_keyboard_choices(choice_model(data::search(
        &lists.data.keyboards,
        &app.get_keyboard_query(),
    )));
    app.set_timezone_choices(choice_model(data::search(
        &lists.data.timezones,
        &app.get_timezone_query(),
    )));
    let locale = app.get_locale().to_string();
    pick_locale(app, &mut lists, &locale);
}

/// The language, and what it suggests for the keyboard and timezone
/// (SPEC.md: the timezone is "guessed from the chosen locale"), unless
/// the user already picked those.
fn pick_locale(app: &AppWindow, lists: &mut Lists, locale: &str) {
    let data = &lists.data;
    app.set_locale(locale.into());
    app.set_locale_label(Data::label(&data.locales, locale).unwrap_or(locale).into());
    if !lists.keyboard_picked
        && let Some(keyboard) = data.keyboard_for_locale(locale)
    {
        show_keyboard(app, data, &keyboard);
    }
    if !lists.timezone_picked
        && let Some(timezone) = data.timezone_for_locale(locale)
    {
        show_timezone(app, data, &timezone);
    }
}

fn show_keyboard(app: &AppWindow, data: &Data, value: &str) {
    let (layout, variant) = data::split_keyboard(value);
    app.set_kb_layout(layout.into());
    app.set_kb_variant(variant.into());
    app.set_keyboard_label(Data::label(&data.keyboards, value).unwrap_or(value).into());
}

fn show_timezone(app: &AppWindow, data: &Data, value: &str) {
    app.set_timezone(value.into());
    app.set_timezone_label(Data::label(&data.timezones, value).unwrap_or(value).into());
}

/// Searching and picking on the Welcome, Keyboard and Timezone screens.
fn wire_lists(app: &AppWindow, lists: &Arc<Mutex<Lists>>) {
    app.on_locale_query_edited({
        let weak = app.as_weak();
        let lists = Arc::clone(lists);
        move |query| {
            if let (Some(app), Ok(lists)) = (weak.upgrade(), lists.lock()) {
                app.set_locale_choices(choice_model(data::search(&lists.data.locales, &query)));
            }
        }
    });
    app.on_locale_picked({
        let weak = app.as_weak();
        let lists = Arc::clone(lists);
        move |choice| {
            if let (Some(app), Ok(mut lists)) = (weak.upgrade(), lists.lock()) {
                pick_locale(&app, &mut lists, &choice.value);
            }
        }
    });

    app.on_keyboard_query_edited({
        let weak = app.as_weak();
        let lists = Arc::clone(lists);
        move |query| {
            if let (Some(app), Ok(lists)) = (weak.upgrade(), lists.lock()) {
                app.set_keyboard_choices(choice_model(data::search(&lists.data.keyboards, &query)));
            }
        }
    });
    app.on_keyboard_picked({
        let weak = app.as_weak();
        let lists = Arc::clone(lists);
        move |choice| {
            if let (Some(app), Ok(mut lists)) = (weak.upgrade(), lists.lock()) {
                lists.keyboard_picked = true;
                show_keyboard(&app, &lists.data, &choice.value);
            }
        }
    });

    app.on_timezone_query_edited({
        let weak = app.as_weak();
        let lists = Arc::clone(lists);
        move |query| {
            if let (Some(app), Ok(lists)) = (weak.upgrade(), lists.lock()) {
                app.set_timezone_choices(choice_model(data::search(&lists.data.timezones, &query)));
            }
        }
    });
    app.on_timezone_picked({
        let weak = app.as_weak();
        let lists = Arc::clone(lists);
        move |choice| {
            if let (Some(app), Ok(mut lists)) = (weak.upgrade(), lists.lock()) {
                lists.timezone_picked = true;
                show_timezone(&app, &lists.data, &choice.value);
            }
        }
    });
}

fn disk_model(disks: Vec<protocol::DiskInfo>) -> ModelRc<DiskInfo> {
    const GIB: u64 = 1024 * 1024 * 1024;
    let rows: Vec<DiskInfo> = disks
        .into_iter()
        .map(|disk| DiskInfo {
            device: disk.device.into(),
            model: disk.model.into(),
            size_gib: i32::try_from(disk.size_bytes / GIB).unwrap_or(i32::MAX),
            serial: disk.serial.into(),
            unavailable: disk.unavailable.unwrap_or_default().into(),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// Auto-fills username and hostname from the full name, but only until
/// the user edits one directly — SPEC.md's Account screen calls both
/// "derived from full name, editable".
fn wire_account_derivation(app: &AppWindow) {
    let username_touched = Rc::new(Cell::new(false));
    let hostname_touched = Rc::new(Cell::new(false));

    app.on_full_name_edited({
        let app_weak = app.as_weak();
        let username_touched = username_touched.clone();
        let hostname_touched = hostname_touched.clone();
        move |full_name| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let username = state::derive_username(&full_name);
            if !username_touched.get() {
                app.set_username(username.clone().into());
            }
            if !hostname_touched.get() {
                app.set_hostname(state::derive_hostname(&username).into());
            }
        }
    });

    app.on_username_edited({
        let username_touched = username_touched.clone();
        move |_| username_touched.set(true)
    });

    app.on_hostname_edited(move |_| hostname_touched.set(true));
}

fn scan_wifi(app: &AppWindow, services: &Services) {
    app.set_wifi_busy(true);
    app.set_wifi_status(WifiStatus::Scanning);
    let weak = app.as_weak();
    let wifi = Arc::clone(&services.wifi);
    std::thread::spawn(move || {
        let result = wifi.scan();
        let _ = weak.upgrade_in_event_loop(move |app| {
            app.set_wifi_busy(false);
            match result {
                Ok(networks) => {
                    app.set_wifi_status(WifiStatus::Idle);
                    let rows: Vec<WifiNetwork> = networks
                        .into_iter()
                        .map(|network| WifiNetwork {
                            ssid: network.ssid.into(),
                            strength: i32::from(network.strength),
                            secured: network.secured,
                        })
                        .collect();
                    app.set_wifi_networks(ModelRc::new(VecModel::from(rows)));
                }
                Err(err) => {
                    app.set_wifi_status(WifiStatus::Failed);
                    app.set_wifi_status_detail(err.into());
                }
            }
        });
    });
}

/// Rescan and Join on the Network screen. After joining, the backend
/// checks the repositories again: the install goes online only if they
/// answer now, and the joined network is set up on the new system either
/// way.
fn wire_wifi(app: &AppWindow, services: &Services) {
    app.on_wifi_rescan({
        let weak = app.as_weak();
        let services = services.clone();
        move || {
            if let Some(app) = weak.upgrade() {
                scan_wifi(&app, &services);
            }
        }
    });

    app.on_wifi_join({
        let weak = app.as_weak();
        let services = services.clone();
        move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let ssid = app.get_selected_ssid().to_string();
            let password = app.get_wifi_password().to_string();
            app.set_wifi_busy(true);
            app.set_wifi_status(WifiStatus::Connecting);
            app.set_wifi_status_detail(ssid.clone().into());
            let weak = app.as_weak();
            let services = services.clone();
            std::thread::spawn(move || {
                let joined = services.wifi.connect(&ssid, &password);
                let online = joined.is_ok() && services.backend.check_online().unwrap_or(false);
                let _ = weak.upgrade_in_event_loop(move |app| {
                    app.set_wifi_busy(false);
                    match joined {
                        Ok(profile) => {
                            app.set_wifi_status(WifiStatus::Idle);
                            app.set_network_profile(profile.into());
                            app.set_is_offline(!online);
                            app.invoke_go_next();
                        }
                        Err(ConnectError::Failed) => app.set_wifi_status(WifiStatus::WrongPassword),
                        Err(ConnectError::TimedOut) => app.set_wifi_status(WifiStatus::TimedOut),
                        Err(ConnectError::Other(message)) => {
                            app.set_wifi_status(WifiStatus::Failed);
                            app.set_wifi_status_detail(message.into());
                        }
                    }
                });
            });
        }
    });
}

fn plan_fields(app: &AppWindow, packages: &[String]) -> state::PlanFields {
    state::PlanFields {
        locale: app.get_locale().to_string(),
        kb_layout: app.get_kb_layout().to_string(),
        kb_variant: app.get_kb_variant().to_string(),
        timezone: app.get_timezone().to_string(),
        hostname: app.get_hostname().to_string(),
        online: !app.get_is_offline(),
        network_profile: app.get_network_profile().to_string(),
        packages: packages.to_vec(),
        disk_device: app.get_selected_device().to_string(),
        secure_boot_enroll: app.get_is_setup_mode() && app.get_secure_boot_enroll(),
        full_name: app.get_full_name().to_string(),
        username: app.get_username().to_string(),
        password: app.get_password().to_string(),
        autologin: app.get_autologin_option() && app.get_autologin(),
    }
}

/// The Summary screen's confirm: the backend validates the plan first
/// (its own rules, and whether the disk is still one it can install
/// onto), then the install starts.
fn wire_install(
    app: &AppWindow,
    services: &Services,
    session: &Arc<Mutex<Session>>,
    packages: Vec<String>,
) {
    let weak = app.as_weak();
    let services = services.clone();
    let session = Arc::clone(session);
    app.on_install_confirmed(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let plan = state::build_install_plan(&plan_fields(&app, &packages));
        let problems = match services.backend.validate(&plan) {
            Ok(errors) => errors,
            Err(err) => vec![err],
        };
        if !problems.is_empty() {
            app.set_summary_problems(problems.join("\n").into());
            return;
        }
        app.set_summary_problems(SharedString::new());
        start_install(&app, &services, &session, plan);
    });
}

fn start_install(
    app: &AppWindow,
    services: &Services,
    session: &Arc<Mutex<Session>>,
    plan: InstallPlan,
) {
    if let Ok(mut session) = session.lock() {
        session.plan = Some(plan.clone());
        session.log.clear();
    }
    let log_model = app.get_install_log();
    if let Some(lines) = log_model.as_any().downcast_ref::<VecModel<SharedString>>() {
        lines.set_vec(Vec::new());
    }
    app.set_install_step_number(0);
    app.set_install_step_name(SharedString::new());
    app.set_install_progress(0.0);
    app.set_saved_log_path(SharedString::new());
    app.set_save_log_error(SharedString::new());
    app.set_current_screen(7);

    let weak = app.as_weak();
    let event_session = Arc::clone(session);
    let on_event: EventSink = Arc::new(move |event: Event| {
        if let Event::Log { line } = &event
            && let Ok(mut session) = event_session.lock()
        {
            session.log.push(line.clone());
        }
        let _ = weak.upgrade_in_event_loop(move |app| apply_event(&app, event));
    });
    if let Err(message) = services.backend.install(&plan, on_event) {
        show_error(app, 0, message, Vec::new(), false);
    }
}

fn apply_event(app: &AppWindow, event: Event) {
    match event {
        Event::Progress {
            step,
            name,
            percent,
        } => {
            app.set_install_step_number(i32::try_from(step).unwrap_or(0));
            app.set_install_step_name(name.into());
            app.set_install_progress(percent / 100.0);
        }
        Event::Log { line } => {
            let model = app.get_install_log();
            if let Some(lines) = model.as_any().downcast_ref::<VecModel<SharedString>>() {
                if lines.row_count() >= SHOWN_LOG_LINES {
                    lines.remove(0);
                }
                lines.push(line.into());
            }
        }
        Event::Done => {
            app.set_install_progress(1.0);
            app.set_current_screen(8);
        }
        Event::Error {
            step,
            message,
            log_tail,
            offline_fallback,
        } => {
            // A backend going away is only news while an install is
            // running; after Done, it's just the machine rebooting.
            if step.is_none() && app.get_current_screen() != 7 {
                return;
            }
            let step = step.and_then(|step| i32::try_from(step).ok()).unwrap_or(0);
            show_error(app, step, message, log_tail, offline_fallback);
        }
        Event::Disks { .. }
        | Event::Firmware { .. }
        | Event::Online { .. }
        | Event::Validation { .. } => {}
    }
}

fn show_error(
    app: &AppWindow,
    step: i32,
    message: String,
    log_tail: Vec<String>,
    offline_fallback: bool,
) {
    app.set_error_step(step);
    app.set_error_message(message.into());
    app.set_error_log(log_tail.join("\n").into());
    app.set_error_offline_fallback(offline_fallback);
    app.set_current_screen(9);
}

/// Retry from start, Install offline instead, and Save log.
fn wire_error_screen(app: &AppWindow, services: &Services, session: &Arc<Mutex<Session>>) {
    app.on_retry_install({
        let weak = app.as_weak();
        let services = services.clone();
        let session = Arc::clone(session);
        move || {
            let plan = session.lock().ok().and_then(|session| session.plan.clone());
            if let (Some(app), Some(plan)) = (weak.upgrade(), plan) {
                start_install(&app, &services, &session, plan);
            }
        }
    });

    app.on_install_offline({
        let weak = app.as_weak();
        let services = services.clone();
        let session = Arc::clone(session);
        move || {
            let plan = session.lock().ok().and_then(|session| session.plan.clone());
            if let (Some(app), Some(mut plan)) = (weak.upgrade(), plan) {
                plan.source = Source::Squashfs;
                app.set_is_offline(true);
                start_install(&app, &services, &session, plan);
            }
        }
    });

    app.on_save_log({
        let weak = app.as_weak();
        let session = Arc::clone(session);
        let dir = services.save_log_dir.clone();
        move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let log = session
                .lock()
                .map(|session| session.log.join("\n") + "\n")
                .unwrap_or_default();
            let path = dir.join("dawn-install.log");
            match std::fs::write(&path, log) {
                Ok(()) => {
                    app.set_saved_log_path(path.display().to_string().into());
                    app.set_save_log_error(SharedString::new());
                }
                Err(err) => app.set_save_log_error(err.to_string().into()),
            }
        }
    });

    app.on_quit_requested(|| {
        let _ = slint::quit_event_loop();
    });
}

fn wire_done_screen(app: &AppWindow) {
    let weak = app.as_weak();
    app.on_restart_requested(move || {
        if let Err(err) = system::reboot()
            && let Some(app) = weak.upgrade()
        {
            app.set_restart_error(err.into());
        }
    });
}
