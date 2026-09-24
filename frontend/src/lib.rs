// SPDX-License-Identifier: GPL-3.0-or-later

//! `dawn`: the Slint GUI. Reads branding and (mock, for now) backend
//! probes, drives the nine screens, and builds an `InstallPlan` —
//! see SPEC.md "Architecture". The real Unix-socket backend arrives in
//! M3; this binary runs entirely in mock-backend mode.
//!
//! Split into a library so the UI smoke test (`tests/clickthrough.rs`)
//! can drive the same `AppWindow` the real binary (`src/main.rs`) runs,
//! rather than duplicating the wiring.

pub mod branding;
pub mod mock_backend;
pub mod state;

use std::cell::Cell;
use std::rc::Rc;

use slint::{ModelRc, VecModel};

slint::include_modules!();

/// The stable identifier SPEC.md's Hyprland window rule matches on
/// ("a stable window title and app_id"). Slint's winit backend doesn't
/// set one on its own (confirmed by inspecting a running window with
/// `hyprctl clients` — its `class` came back empty) — see DECISIONS.md.
pub const APP_ID: &str = "luminos-dawn";

/// Builds an `AppWindow` with branding loaded, mock backend data filled
/// in, and every callback wired — everything `main()` needs before
/// calling `.run()`, and everything the UI smoke test needs before
/// driving it.
pub fn build_ui() -> Result<AppWindow, slint::PlatformError> {
    let app = AppWindow::new()?;
    // Errors here just mean no Wayland/X11 platform is present (the
    // testing backend, say) — harmless to ignore per set_xdg_app_id's
    // own doc comment.
    let _ = slint::set_xdg_app_id(APP_ID);

    let branding_dir = branding::find_branding_dir("luminos");
    match branding::load(&branding_dir) {
        Ok(branding) => apply_branding(&app, branding),
        Err(err) => eprintln!(
            "warning: could not load branding from {}: {err}",
            branding_dir.display()
        ),
    }

    let firmware = mock_backend::probe_firmware();
    app.set_is_uefi(firmware.is_uefi);
    app.set_is_setup_mode(firmware.is_setup_mode);
    app.set_secure_boot_enroll(firmware.is_setup_mode);
    app.set_is_offline(!mock_backend::check_online());

    let disks: Vec<DiskInfo> = mock_backend::list_disks()
        .into_iter()
        .map(|disk| DiskInfo {
            device: disk.device.into(),
            model: disk.model.into(),
            size_gib: disk.size_gib,
            serial: disk.serial.into(),
        })
        .collect();
    app.set_disks(ModelRc::new(VecModel::from(disks)));

    wire_account_derivation(&app);
    wire_install_confirmed(&app);
    wire_quit(&app);

    Ok(app)
}

pub fn apply_branding(app: &AppWindow, branding: branding::Branding) {
    let theme = app.global::<Theme>();
    theme.set_product_name(branding.product_name.into());
    theme.set_version_string(branding.version_string.into());
    theme.set_accent(branding.accent_color);
    theme.set_website(branding.website.into());
    theme.set_support_url(branding.support_url.into());
    theme.set_logo(branding.logo.clone());
    theme.set_icon(branding.icon);
    theme.set_welcome_text(branding.welcome_text.into());
}

/// Auto-fills username and hostname from the full name, but only until
/// the user edits one directly — SPEC.md's Account screen calls both
/// "derived from full name, editable".
pub fn wire_account_derivation(app: &AppWindow) {
    let username_touched = Rc::new(Cell::new(false));
    let hostname_touched = Rc::new(Cell::new(false));

    app.on_full_name_edited({
        let app_weak = app.as_weak();
        let username_touched = username_touched.clone();
        let hostname_touched = hostname_touched.clone();
        move |full_name| {
            let app = app_weak.unwrap();
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

pub fn wire_install_confirmed(app: &AppWindow) {
    app.on_install_confirmed({
        let app_weak = app.as_weak();
        move || {
            let app = app_weak.unwrap();
            let fields = state::PlanFields {
                locale: app.get_locale().to_string(),
                kb_layout: app.get_kb_layout().to_string(),
                kb_variant: app.get_kb_variant().to_string(),
                timezone: app.get_timezone().to_string(),
                hostname: app.get_hostname().to_string(),
                is_offline: app.get_is_offline(),
                network_name: app.get_network_name().to_string(),
                disk_device: app.get_selected_device().to_string(),
                secure_boot_enroll: app.get_secure_boot_enroll(),
                full_name: app.get_full_name().to_string(),
                username: app.get_username().to_string(),
                password: app.get_password().to_string(),
                autologin: app.get_autologin(),
            };
            let install_plan = state::build_install_plan(&fields);
            match serde_json::to_string_pretty(&install_plan) {
                Ok(json) => app.set_plan_json(json.into()),
                Err(err) => eprintln!("error: could not serialize the install plan: {err}"),
            }
        }
    });
}

pub fn wire_quit(app: &AppWindow) {
    app.on_quit_requested(|| {
        let _ = slint::quit_event_loop();
    });
}
