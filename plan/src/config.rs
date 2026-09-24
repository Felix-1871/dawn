// SPDX-License-Identifier: GPL-3.0-or-later

//! `installer.toml`: Dawn's behaviour config (SPEC.md "Branding &
//! configuration"), read by both the GUI and the backend. Types and
//! parsing only; reading the file is each binary's job.

use serde::Deserialize;
use thiserror::Error;

use crate::install_plan::Filesystem;

/// Where a real system keeps the config.
pub const SYSTEM_PATH: &str = "/etc/dawn/installer.toml";

/// The example shipped in `config/installer.toml`, compiled in. The
/// binaries fall back to it when [`SYSTEM_PATH`] doesn't exist, which is
/// also what keeps `dawn-backend --dry-run` working on a development
/// machine.
pub const DEFAULT_TOML: &str = include_str!("../../config/installer.toml");

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallerConfig {
    /// Folder name under `/usr/share/dawn/branding/`.
    pub branding: String,
    /// Which backend adapter handles the distro-specific steps. Only
    /// `arch` exists in v1.
    pub adapter: String,
    pub source: SourceConfig,
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub screens: ScreensConfig,
    #[serde(default)]
    pub offline_cleanup: OfflineCleanupConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    /// What an online install pacstraps; the GUI copies it into the plan.
    pub packages: Vec<String>,
    /// The live image an offline install unsquashes, on the boot medium.
    pub squashfs: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultsConfig {
    pub filesystem: Filesystem,
    pub admin_group: String,
    pub min_disk_gib: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreensConfig {
    /// Whether the Account screen offers automatic login.
    #[serde(default = "default_true")]
    pub autologin_option: bool,
}

impl Default for ScreensConfig {
    fn default() -> Self {
        Self {
            autologin_option: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfflineCleanupConfig {
    /// Removed with `pacman -Rns` after an offline install, next to Dawn's
    /// own package, which the adapter always removes.
    #[serde(default)]
    pub remove_packages: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{0}")]
    Parse(String),
    #[error("adapter {0:?} is not supported (only \"arch\" is)")]
    UnsupportedAdapter(String),
    #[error("[source] packages must not be empty")]
    NoPackages,
    #[error("[source] squashfs {0:?} must be an absolute path")]
    SquashfsNotAbsolute(String),
    #[error("[defaults] min_disk_gib must be more than 0")]
    ZeroMinDisk,
}

impl InstallerConfig {
    pub fn from_toml_str(raw: &str) -> Result<Self, ConfigError> {
        let config: Self =
            toml::from_str(raw).map_err(|err| ConfigError::Parse(err.to_string()))?;
        config.check()?;
        Ok(config)
    }

    /// The compiled-in [`DEFAULT_TOML`].
    pub fn builtin_default() -> Result<Self, ConfigError> {
        Self::from_toml_str(DEFAULT_TOML)
    }

    fn check(&self) -> Result<(), ConfigError> {
        if self.adapter != "arch" {
            return Err(ConfigError::UnsupportedAdapter(self.adapter.clone()));
        }
        if self.source.packages.is_empty() {
            return Err(ConfigError::NoPackages);
        }
        if !self.source.squashfs.starts_with('/') {
            return Err(ConfigError::SquashfsNotAbsolute(
                self.source.squashfs.clone(),
            ));
        }
        if self.defaults.min_disk_gib == 0 {
            return Err(ConfigError::ZeroMinDisk);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_example_parses() {
        let config = InstallerConfig::builtin_default().unwrap();
        assert_eq!(config.branding, "luminos");
        assert_eq!(config.source.packages, ["luminos-base", "luminos-desktop"]);
        assert_eq!(config.defaults.min_disk_gib, 32);
        assert_eq!(config.defaults.filesystem, Filesystem::Btrfs);
        assert!(config.screens.autologin_option);
        assert_eq!(
            config.offline_cleanup.remove_packages,
            ["mkinitcpio-archiso"]
        );
    }

    #[test]
    fn optional_tables_default() {
        let config = InstallerConfig::from_toml_str(
            r#"
            branding = "luminos"
            adapter = "arch"
            [source]
            packages = ["base"]
            squashfs = "/run/archiso/bootmnt/arch/x86_64/airootfs.sfs"
            [defaults]
            filesystem = "btrfs"
            admin_group = "wheel"
            min_disk_gib = 20
            "#,
        )
        .unwrap();
        assert!(config.screens.autologin_option);
        assert!(config.offline_cleanup.remove_packages.is_empty());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let raw = DEFAULT_TOML.replace("min_disk_gib = 32", "min_disk_gib = 32\nmin_disk_gb = 1");
        assert!(matches!(
            InstallerConfig::from_toml_str(&raw),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn only_the_arch_adapter_exists() {
        let raw = DEFAULT_TOML.replace("adapter = \"arch\"", "adapter = \"debian\"");
        assert_eq!(
            InstallerConfig::from_toml_str(&raw),
            Err(ConfigError::UnsupportedAdapter("debian".into()))
        );
    }

    #[test]
    fn a_relative_squashfs_path_is_rejected() {
        let raw = DEFAULT_TOML.replace(
            "/run/archiso/bootmnt/arch/x86_64/airootfs.sfs",
            "arch/x86_64/airootfs.sfs",
        );
        assert!(matches!(
            InstallerConfig::from_toml_str(&raw),
            Err(ConfigError::SquashfsNotAbsolute(_))
        ));
    }
}
