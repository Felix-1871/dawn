// SPDX-License-Identifier: GPL-3.0-or-later

//! M2's own Done-when check (SPEC.md): "Clicking through produces a
//! valid plan identical to a hand-written one." Drives all nine
//! screens through `build_ui()` — the same function `main()` calls —
//! using Slint's testing backend.
//!
//! Navigation (Next/Back/Install) is driven the way SPEC.md describes
//! these smoke tests: by accessible label, through the same
//! accessibility action a screen reader would use. Form fields backed
//! by a ComboBox are set directly instead, since std-widgets'
//! ComboBox doesn't expose an accessible set-value action (only
//! LineEdit and Button/CheckBox do) — SPEC.md calls out driving
//! Next/Back specifically, not every widget kind.

use i_slint_backend_testing::ElementHandle;

fn click(app: &frontend::AppWindow, label: &str) {
    let mut matches = ElementHandle::find_by_accessible_label(app, label);
    let handle = matches
        .next()
        .unwrap_or_else(|| panic!("no element with accessible label {label:?}"));
    assert!(
        matches.next().is_none(),
        "more than one element labeled {label:?} on screen {}",
        app.get_current_screen()
    );
    handle.invoke_accessible_default_action();
}

#[test]
fn clicking_through_produces_the_same_plan_as_the_fixture() {
    i_slint_backend_testing::init_no_event_loop();

    let app = frontend::build_ui().expect("build_ui should succeed in the testing backend");

    // Welcome: the mock firmware probe reports UEFI, so Install is
    // enabled; the mock locale combo box only offers en_US.UTF-8,
    // which is already the default.
    assert_eq!(app.get_current_screen(), 0);
    click(&app, "Install");

    // The mock check_online() reports online, so Network is skipped
    // straight to Keyboard.
    assert_eq!(app.get_current_screen(), 2);
    app.set_kb_layout("us".into());
    app.set_kb_variant("".into());
    click(&app, "Next");

    assert_eq!(app.get_current_screen(), 3);
    app.set_timezone("Europe/Berlin".into());
    click(&app, "Next");

    // Disk: select the one mock disk (see mock_backend::list_disks).
    assert_eq!(app.get_current_screen(), 4);
    app.set_selected_device("/dev/disk/by-id/nvme-EXAMPLE_SERIAL".into());
    app.set_selected_model("EXAMPLE SSD 512GB".into());
    click(&app, "Next");

    // Account: full name drives the real derivation logic via the
    // same callback a LineEdit's `edited` would fire; hostname is then
    // edited by hand, exactly as SPEC.md says both fields allow.
    assert_eq!(app.get_current_screen(), 5);
    app.set_full_name("Ada Lovelace".into());
    app.invoke_full_name_edited("Ada Lovelace".into());
    assert_eq!(app.get_username(), "ada");
    app.set_hostname("ada-laptop".into());
    app.invoke_hostname_edited("ada-laptop".into());
    app.set_password("correct-horse-battery-staple".into());
    click(&app, "Next");

    // Summary: nothing touches a disk before this confirm.
    assert_eq!(app.get_current_screen(), 6);
    click(&app, "Install");

    assert_eq!(
        app.get_current_screen(),
        7,
        "confirming should move to Installing"
    );

    let produced: serde_json::Value =
        serde_json::from_str(&app.get_plan_json()).expect("plan_json should be valid JSON");

    let fixture_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/plans/erase-online.json"
    );
    let fixture_raw = std::fs::read_to_string(fixture_path).expect("read the fixture plan");
    let expected: serde_json::Value =
        serde_json::from_str(&fixture_raw).expect("parse the fixture plan");

    assert_eq!(
        produced, expected,
        "the clicked-through plan should be identical to tests/plans/erase-online.json"
    );
}

#[test]
fn back_returns_to_welcome_from_keyboard() {
    i_slint_backend_testing::init_no_event_loop();
    let app = frontend::build_ui().expect("build_ui should succeed in the testing backend");

    click(&app, "Install");
    assert_eq!(app.get_current_screen(), 2);
    click(&app, "Back");
    assert_eq!(
        app.get_current_screen(),
        0,
        "going back from Keyboard should skip Network when online, same as going forward did"
    );
}
