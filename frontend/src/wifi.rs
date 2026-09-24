// SPDX-License-Identifier: GPL-3.0-or-later

//! Wi-Fi for the Network screen, through NetworkManager's D-Bus API. The
//! GUI does this itself (SPEC.md "Architecture": "The frontend talks to
//! NetworkManager over D-Bus itself to scan and join Wi-Fi"); the backend
//! only copies the joined network's saved profile into the target
//! (step 7). The profile is saved system-wide with its password, so that
//! copy works — LuminOS has to let the live user do that without a
//! password prompt (see LUMINOS-CHANGES.md).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

pub struct Network {
    pub ssid: String,
    /// 0 to 100.
    pub strength: u8,
    pub secured: bool,
}

/// Why joining a network didn't work. The Network screen words these
/// itself, so its text goes through `@tr`.
#[derive(Debug, PartialEq, Eq)]
pub enum ConnectError {
    /// NetworkManager tried and gave up: almost always a wrong password.
    Failed,
    TimedOut,
    /// Anything else, as the system described it.
    Other(String),
}

pub trait Wifi: Send + Sync {
    /// Whether there's a Wi-Fi adapter to use at all.
    fn available(&self) -> bool;
    /// Networks in range, strongest first, one entry per name.
    fn scan(&self) -> Result<Vec<Network>, String>;
    /// Joins `ssid` and returns the saved profile's name, which step 7
    /// copies into the installed system.
    fn connect(&self, ssid: &str, password: &str) -> Result<String, ConnectError>;
}

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const SETTINGS_CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";

/// NM_DEVICE_TYPE_WIFI.
const DEVICE_TYPE_WIFI: u32 = 2;
/// NM_ACTIVE_CONNECTION_STATE_ACTIVATED and _DEACTIVATED.
const STATE_ACTIVATED: u32 = 2;
const STATE_DEACTIVATED: u32 = 4;
/// How long NetworkManager gets to find the network, authenticate and
/// get an address.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

/// The real thing, on the system bus.
pub struct NetworkManager {
    bus: Option<Connection>,
}

impl NetworkManager {
    /// Never fails: with no system bus or no NetworkManager, there's
    /// simply no Wi-Fi (the Network screen says so and offers Skip).
    pub fn new() -> Self {
        Self {
            bus: Connection::system().ok(),
        }
    }

    fn bus(&self) -> Result<&Connection, String> {
        self.bus
            .as_ref()
            .ok_or_else(|| "The system bus isn't available.".to_string())
    }

    fn property(&self, path: &str, interface: &str, name: &str) -> Result<OwnedValue, String> {
        let reply = self
            .bus()?
            .call_method(Some(NM), path, Some(PROPERTIES), "Get", &(interface, name))
            .map_err(|err| err.to_string())?;
        reply.body().deserialize().map_err(|err| err.to_string())
    }

    fn u32_property(&self, path: &str, interface: &str, name: &str) -> Result<u32, String> {
        u32::try_from(self.property(path, interface, name)?).map_err(|err| err.to_string())
    }

    fn wifi_device(&self) -> Result<Option<OwnedObjectPath>, String> {
        let reply = self
            .bus()?
            .call_method(Some(NM), NM_PATH, Some(NM), "GetDevices", &())
            .map_err(|err| err.to_string())?;
        let devices: Vec<OwnedObjectPath> =
            reply.body().deserialize().map_err(|err| err.to_string())?;
        for device in devices {
            if self.u32_property(device.as_str(), DEVICE, "DeviceType")? == DEVICE_TYPE_WIFI {
                return Ok(Some(device));
            }
        }
        Ok(None)
    }

    fn access_point(&self, path: &str) -> Result<Network, String> {
        let ssid_bytes = Vec::<u8>::try_from(self.property(path, ACCESS_POINT, "Ssid")?)
            .map_err(|err| err.to_string())?;
        let strength = u8::try_from(self.property(path, ACCESS_POINT, "Strength")?)
            .map_err(|err| err.to_string())?;
        let flags = self.u32_property(path, ACCESS_POINT, "Flags")?;
        let wpa = self.u32_property(path, ACCESS_POINT, "WpaFlags")?;
        let rsn = self.u32_property(path, ACCESS_POINT, "RsnFlags")?;
        Ok(Network {
            ssid: String::from_utf8_lossy(&ssid_bytes).into_owned(),
            strength,
            secured: flags & 1 != 0 || wpa != 0 || rsn != 0,
        })
    }

    fn access_points(&self, device: &OwnedObjectPath) -> Result<Vec<OwnedObjectPath>, String> {
        let reply = self
            .bus()?
            .call_method(
                Some(NM),
                device.as_str(),
                Some(WIRELESS),
                "GetAllAccessPoints",
                &(),
            )
            .map_err(|err| err.to_string())?;
        reply.body().deserialize().map_err(|err| err.to_string())
    }

    /// The strongest access point broadcasting `ssid`, if one is in range.
    fn strongest_access_point(
        &self,
        device: &OwnedObjectPath,
        ssid: &str,
    ) -> Option<OwnedObjectPath> {
        let mut best: Option<(u8, OwnedObjectPath)> = None;
        for path in self.access_points(device).ok()? {
            let Ok(network) = self.access_point(path.as_str()) else {
                continue;
            };
            if network.ssid == ssid
                && best
                    .as_ref()
                    .is_none_or(|(strength, _)| network.strength > *strength)
            {
                best = Some((network.strength, path));
            }
        }
        best.map(|(_, path)| path)
    }

    fn delete_connection(&self, path: &str) {
        if let Ok(bus) = self.bus() {
            let _ = bus.call_method(Some(NM), path, Some(SETTINGS_CONNECTION), "Delete", &());
        }
    }
}

impl Default for NetworkManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Wifi for NetworkManager {
    fn available(&self) -> bool {
        matches!(self.wifi_device(), Ok(Some(_)))
    }

    fn scan(&self) -> Result<Vec<Network>, String> {
        let device = self
            .wifi_device()?
            .ok_or_else(|| "No Wi-Fi adapter was found.".to_string())?;
        // A scan NetworkManager refuses (one ran moments ago, say) still
        // leaves the last results to read.
        let empty: HashMap<&str, Value> = HashMap::new();
        if self
            .bus()?
            .call_method(
                Some(NM),
                device.as_str(),
                Some(WIRELESS),
                "RequestScan",
                &(empty,),
            )
            .is_ok()
        {
            std::thread::sleep(Duration::from_secs(3));
        }
        let mut networks: Vec<Network> = Vec::new();
        for path in self.access_points(&device)? {
            let Ok(network) = self.access_point(path.as_str()) else {
                continue;
            };
            if network.ssid.is_empty() {
                continue;
            }
            match networks.iter_mut().find(|n| n.ssid == network.ssid) {
                Some(known) if known.strength >= network.strength => {}
                Some(known) => *known = network,
                None => networks.push(network),
            }
        }
        networks.sort_by_key(|network| std::cmp::Reverse(network.strength));
        Ok(networks)
    }

    fn connect(&self, ssid: &str, password: &str) -> Result<String, ConnectError> {
        let device = self
            .wifi_device()
            .map_err(ConnectError::Other)?
            .ok_or_else(|| ConnectError::Other("no Wi-Fi adapter was found".to_string()))?;

        let mut connection: HashMap<&str, Value> = HashMap::new();
        connection.insert("id", Value::from(ssid));
        connection.insert("type", Value::from("802-11-wireless"));
        let mut wireless: HashMap<&str, Value> = HashMap::new();
        wireless.insert("ssid", Value::from(ssid.as_bytes().to_vec()));
        wireless.insert("mode", Value::from("infrastructure"));
        let mut settings: HashMap<&str, HashMap<&str, Value>> = HashMap::new();
        settings.insert("connection", connection);
        settings.insert("802-11-wireless", wireless);
        // Given the access point, NetworkManager fills in the key
        // management it supports (WPA2's wpa-psk, WPA3's sae), as it does
        // for `nmcli device wifi connect`; without one, WPA2 is assumed.
        let access_point = self.strongest_access_point(&device, ssid);
        if !password.is_empty() {
            let mut security: HashMap<&str, Value> = HashMap::new();
            if access_point.is_none() {
                security.insert("key-mgmt", Value::from("wpa-psk"));
            }
            security.insert("psk", Value::from(password));
            // Stored in the system-wide profile, not by a desktop secret
            // agent: the live session has none, and the installed system
            // needs the password in the file step 7 copies.
            security.insert("psk-flags", Value::from(0u32));
            settings.insert("802-11-wireless-security", security);
        }
        let other = |err: &dyn std::fmt::Display| ConnectError::Other(err.to_string());
        let specific_object = match &access_point {
            Some(path) => ObjectPath::try_from(path.as_str()),
            None => ObjectPath::try_from("/"),
        }
        .map_err(|err| other(&err))?;
        let device_path = ObjectPath::try_from(device.as_str()).map_err(|err| other(&err))?;

        let reply = self
            .bus()
            .map_err(|err| other(&err))?
            .call_method(
                Some(NM),
                NM_PATH,
                Some(NM),
                "AddAndActivateConnection",
                &(settings, device_path, specific_object),
            )
            .map_err(|err| other(&err))?;
        let (profile, active): (OwnedObjectPath, OwnedObjectPath) =
            reply.body().deserialize().map_err(|err| other(&err))?;

        let started = Instant::now();
        loop {
            match self.u32_property(active.as_str(), ACTIVE, "State") {
                Ok(STATE_ACTIVATED) => break,
                // A failed activation's object disappears, so a property
                // that can't be read any more means the same.
                Ok(STATE_DEACTIVATED) | Err(_) => {
                    self.delete_connection(profile.as_str());
                    return Err(ConnectError::Failed);
                }
                Ok(_) if started.elapsed() > CONNECT_TIMEOUT => {
                    self.delete_connection(profile.as_str());
                    return Err(ConnectError::TimedOut);
                }
                Ok(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }

        // The keyfile NetworkManager saved is what step 7 copies; its name
        // can differ from the network's (a second "home-wifi" becomes
        // "home-wifi-1.nmconnection", say).
        let filename = self
            .property(profile.as_str(), SETTINGS_CONNECTION, "Filename")
            .ok()
            .and_then(|value| String::try_from(value).ok())
            .unwrap_or_default();
        let stem = std::path::Path::new(&filename)
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".nmconnection"))
            .map(str::to_string);
        Ok(stem.unwrap_or_else(|| ssid.to_string()))
    }
}
