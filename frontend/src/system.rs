// SPDX-License-Identifier: GPL-3.0-or-later

//! The Done screen's "Restart now" (SPEC.md "User flow"), through logind
//! on the system bus. logind lets the user of an active local session
//! reboot without asking for a password.

pub fn reboot() -> Result<(), String> {
    let bus = zbus::blocking::Connection::system().map_err(|err| err.to_string())?;
    bus.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1.Manager"),
        "Reboot",
        // false: don't prompt; fail instead if a password would be needed.
        &(false,),
    )
    .map(|_| ())
    .map_err(|err| format!("Couldn't restart: {err}"))
}
