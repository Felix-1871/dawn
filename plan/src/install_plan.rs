// SPDX-License-Identifier: GPL-3.0-or-later

//! The `InstallPlan`: the only thing that passes between the Dawn frontend
//! and backend, and the only input the backend CLI accepts. See SPEC.md
//! "Architecture" for the wire protocol this travels over.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::redacted::Redacted;

/// The InstallPlan versions this crate understands. Bump when the shape of
/// the plan changes in a way old plans can't be read as.
pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InstallPlan {
    pub version: u32,
    pub locale: String,
    pub keyboard: Keyboard,
    pub timezone: String,
    pub hostname: String,
    pub source: Source,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_profile: Option<String>,
    pub disk: Disk,
    pub secure_boot: SecureBoot,
    pub user: User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Pacstrap,
    Squashfs,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Keyboard {
    pub layout: String,
    #[serde(default)]
    pub variant: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiskMode {
    Erase,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Filesystem {
    Btrfs,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Disk {
    pub mode: DiskMode,
    /// A `/dev/disk/by-id/...` path — never a raw `/dev/sdX` or `/dev/nvmeXnY`,
    /// which a reboot can reassign to a different physical disk.
    pub device: String,
    pub filesystem: Filesystem,
    /// Only meaningful (and required to be valid) in manual mode: the
    /// existing partitions to reuse, and where each one is mounted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partitions: Option<Vec<PartitionAssignment>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PartitionAssignment {
    /// A `/dev/disk/by-id/...` (or `by-partuuid`) path to one existing
    /// partition on the target disk.
    pub device: String,
    pub mount_point: String,
    pub format: bool,
    pub role: PartitionRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PartitionRole {
    Esp,
    Root,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
pub struct SecureBoot {
    pub enroll: bool,
    pub microsoft_keys: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct User {
    pub full_name: String,
    pub username: String,
    pub password: Redacted,
    #[serde(default)]
    pub autologin: bool,
}
