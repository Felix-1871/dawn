// SPDX-License-Identifier: GPL-3.0-or-later

//! One module per base distro for the handful of pipeline steps that
//! aren't distro-agnostic (see SPEC.md "Install pipeline"'s "Via adapter"
//! column). LuminOS's is `arch`; it's the only one Dawn ships in v1.

pub mod arch;

use plan::InstallPlan;

use crate::runner::Action;

/// Where the target filesystem is being built. Computed once per install
/// from the plan's disk assignment; every adapter method gets the same
/// view of it.
pub struct DiskLayout {
    pub target: String,
    pub esp_device: String,
    pub root_device: String,
    /// The btrfs subvolume mounted at `/`, if root isn't the
    /// filesystem's top level. The kernel mounts the top level unless
    /// told otherwise, so the boot command line has to name it.
    pub root_subvolume: Option<String>,
}

pub trait Adapter {
    fn name(&self) -> &'static str;

    /// Step 5: pacstrap the meta-packages, or unsquash the ISO image.
    fn install_base(&self, plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action>;

    /// Step 6, offline installs only: strip the live-only parts back out.
    fn offline_cleanup(&self, plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action>;

    /// Step 7, the part that's distro-specific: writing the chosen
    /// keyboard layout into the desktop's own config under `/etc/skel`.
    fn keyboard_config(&self, plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action>;

    /// Step 8, the part that's distro-specific: which group grants sudo
    /// (`installer.toml`'s `[defaults] admin_group`).
    fn admin_group(&self) -> &str;

    /// Step 9: mkinitcpio presets and the UKIs they build.
    fn build_ukis(&self, plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action>;

    /// Step 12, the part that's distro-specific: which services this
    /// desktop needs enabled, and autologin if the plan asked for it.
    fn services(&self, plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action>;
}
