// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    // Debug info is what lets the UI smoke test (tests/clickthrough.rs)
    // find elements by accessible label via i-slint-backend-testing.
    let config = slint_build::CompilerConfiguration::new().with_debug_info(true);
    slint_build::compile_with_config("ui/app-window.slint", config)
        .expect("failed to compile ui/app-window.slint");
}
