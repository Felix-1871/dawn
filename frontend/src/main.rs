// SPDX-License-Identifier: GPL-3.0-or-later

//! Entry point. All the actual wiring lives in `lib.rs` so the UI
//! smoke test can reuse it — see that file's doc comment.

use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    frontend::build_ui()?.run()
}
