// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared `InstallPlan` types, JSON schema and validation, plus the other
//! types both binaries share: `installer.toml`'s ([`config`]) and the
//! socket protocol's ([`protocol`]). No I/O: reading plan and config
//! files, probing disks and talking to the backend socket are the
//! frontend's and backend's job, not this crate's. See SPEC.md
//! "Architecture".

pub mod config;
mod install_plan;
pub mod protocol;
mod redacted;
pub mod validate;

pub use install_plan::{
    CURRENT_VERSION, Disk, DiskMode, Filesystem, InstallPlan, Keyboard, PartitionAssignment,
    PartitionRole, SecureBoot, Source, User,
};
pub use redacted::Redacted;
pub use validate::ValidationError;

use schemars::schema::RootSchema;

/// The JSON Schema for [`InstallPlan`], generated from the types themselves
/// so it can never drift from what `serde` actually accepts.
pub fn json_schema() -> RootSchema {
    schemars::schema_for!(InstallPlan)
}
