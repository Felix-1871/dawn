// SPDX-License-Identifier: GPL-3.0-or-later

//! The twelve-step install pipeline from SPEC.md "Install pipeline". This
//! module builds the full command list for a validated plan; running it
//! is `install.rs`'s job.

use plan::{DiskMode, InstallPlan, PartitionAssignment, PartitionRole, Source};
use thiserror::Error;

use crate::adapters::{Adapter, DiskLayout};
use crate::runner::{Action, Capture, Invocation, Stdin};

pub const TARGET: &str = "/mnt/target";

/// The live session's install log (SPEC.md "On failure"), copied into
/// the target by step 12.
pub const LOG_FILE: &str = "/var/log/dawn.log";

/// The pacman config an online install uses: the live system's own,
/// which includes the LuminOS repository (SPEC.md "LuminOS adapter").
/// Step 1 checks its repositories are reachable, and step 5's pacstrap
/// reads it.
pub const PACMAN_CONF: &str = "/etc/pacman.conf";

pub struct PipelineStep {
    pub number: u32,
    pub name: &'static str,
    pub actions: Vec<Action>,
}

/// A plan the pipeline can't be built from. Validation catches these
/// first; this is what's left when a plan skipped it.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PipelineError {
    #[error("step {step}: manual mode needs a partitions list")]
    MissingPartitions { step: u32 },
    #[error("step {step}: manual mode needs a {role} partition")]
    MissingPartition { step: u32, role: &'static str },
}

/// Builds the ordered list of steps for `plan`, delegating the
/// distro-specific parts to `adapter`. Does not run anything.
pub fn build(
    plan: &InstallPlan,
    adapter: &dyn Adapter,
) -> Result<Vec<PipelineStep>, PipelineError> {
    let layout = disk_layout(plan)?;
    let offline = matches!(plan.source, Source::Squashfs);

    let mut steps = vec![
        PipelineStep {
            number: 1,
            name: "Validate plan and re-probe the disk",
            actions: step1_validate(plan),
        },
        PipelineStep {
            number: 2,
            name: "Partition the disk",
            actions: step2_partition(plan),
        },
        PipelineStep {
            number: 3,
            name: "Format and create btrfs subvolumes",
            actions: step3_format(plan, &layout)?,
        },
        PipelineStep {
            number: 4,
            name: "Mount the target",
            actions: step4_mount(plan, &layout)?,
        },
        PipelineStep {
            number: 5,
            name: "Install the base system",
            actions: adapter.install_base(plan, &layout),
        },
    ];

    if offline {
        steps.push(PipelineStep {
            number: 6,
            name: "Remove live-only packages and files",
            actions: adapter.offline_cleanup(plan, &layout),
        });
    }

    steps.push(PipelineStep {
        number: 7,
        name: "Write fstab, locale, keymap, timezone, hostname, zram",
        actions: step7_system_config(plan, &layout, adapter),
    });
    steps.push(PipelineStep {
        number: 8,
        name: "Create the user, lock root, enable sudo",
        actions: step8_user(plan, &layout, adapter),
    });
    steps.push(PipelineStep {
        number: 9,
        name: "Build UKIs",
        actions: adapter.build_ukis(plan, &layout),
    });

    if plan.secure_boot.enroll {
        steps.push(PipelineStep {
            number: 10,
            name: "Secure Boot: create keys, sign, enroll",
            actions: step10_secure_boot(&layout),
        });
    }

    steps.push(PipelineStep {
        number: 11,
        name: "Install the bootloader",
        actions: vec![Action::Run(arch_chroot(&layout, ["bootctl", "install"]))],
    });
    steps.push(PipelineStep {
        number: 12,
        name: "Enable services, copy the log, unmount",
        actions: step12_finish(plan, &layout, adapter),
    });

    Ok(steps)
}

fn manual_partitions(
    plan: &InstallPlan,
    step: u32,
) -> Result<&[PartitionAssignment], PipelineError> {
    plan.disk
        .partitions
        .as_deref()
        .ok_or(PipelineError::MissingPartitions { step })
}

fn disk_layout(plan: &InstallPlan) -> Result<DiskLayout, PipelineError> {
    let target = TARGET.to_string();
    match plan.disk.mode {
        DiskMode::Erase => Ok(DiskLayout {
            target,
            esp_device: format!("{}-part1", plan.disk.device),
            root_device: format!("{}-part2", plan.disk.device),
            root_subvolume: Some("@".to_string()),
        }),
        DiskMode::Manual => {
            // The layout is first needed by step 3.
            let partitions = manual_partitions(plan, 3)?;
            let find = |role, name| {
                partitions
                    .iter()
                    .find(|p| p.role == role)
                    .map(|p| p.device.clone())
                    .ok_or(PipelineError::MissingPartition {
                        step: 3,
                        role: name,
                    })
            };
            // Manual mode mounts the root partition's top level at / in
            // step 4, not a subvolume.
            Ok(DiskLayout {
                target,
                esp_device: find(PartitionRole::Esp, "ESP")?,
                root_device: find(PartitionRole::Root, "root")?,
                root_subvolume: None,
            })
        }
    }
}

fn arch_chroot(
    layout: &DiskLayout,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Invocation {
    let mut full_args = vec![layout.target.clone()];
    full_args.extend(args.into_iter().map(Into::into));
    Invocation::new("arch-chroot", full_args)
}

fn step1_validate(plan: &InstallPlan) -> Vec<Action> {
    let mut actions = vec![Action::Run(Invocation::new(
        "lsblk",
        [
            "--json",
            "-o",
            "NAME,SIZE,MODEL,MOUNTPOINT",
            &plan.disk.device,
        ],
    ))];
    if plan.source == Source::Pacstrap {
        actions.push(Action::CheckReposReachable {
            pacman_conf: PACMAN_CONF.to_string(),
        });
    }
    actions
}

fn step2_partition(plan: &InstallPlan) -> Vec<Action> {
    match plan.disk.mode {
        DiskMode::Erase => {
            let script = "label: gpt\n\
                           size=1GiB, type=uefi, name=\"ESP\"\n\
                           type=linux, name=\"root\""
                .to_string();
            vec![
                Action::Run(
                    Invocation::new("sfdisk", ["--wipe", "always", &plan.disk.device])
                        .with_stdin(Stdin::Plain(script)),
                ),
                Action::Run(Invocation::new("partprobe", [plan.disk.device.as_str()])),
                udev_settle(),
            ]
        }
        DiskMode::Manual => {
            vec![
                Action::Run(Invocation::new("partprobe", [plan.disk.device.as_str()])),
                udev_settle(),
            ]
        }
    }
}

/// The kernel knows about new partitions as soon as partprobe returns,
/// but their `/dev/disk/by-id/...-partN` links come from udev a moment
/// later, and step 3 formats through exactly those paths. Without a
/// running udev (a plain container) this returns at once.
fn udev_settle() -> Action {
    Action::Run(Invocation::new("udevadm", ["settle"]))
}

fn step3_format(plan: &InstallPlan, layout: &DiskLayout) -> Result<Vec<Action>, PipelineError> {
    match plan.disk.mode {
        DiskMode::Erase => Ok(vec![
            Action::Run(Invocation::new(
                "mkfs.fat",
                ["-F32", "-n", "ESP", &layout.esp_device],
            )),
            Action::Run(Invocation::new(
                "mkfs.btrfs",
                ["-f", "-L", "root", &layout.root_device],
            )),
            Action::Run(Invocation::new("mkdir", ["-p", TARGET])),
            Action::Run(Invocation::new(
                "mount",
                [layout.root_device.as_str(), TARGET],
            )),
            Action::Run(Invocation::new(
                "btrfs",
                ["subvolume", "create", &format!("{TARGET}/@")],
            )),
            Action::Run(Invocation::new(
                "btrfs",
                ["subvolume", "create", &format!("{TARGET}/@home")],
            )),
            Action::Run(Invocation::new("umount", [TARGET])),
        ]),
        DiskMode::Manual => {
            let partitions = manual_partitions(plan, 3)?;
            let mut actions = Vec::new();
            for partition in partitions {
                if !partition.format {
                    continue;
                }
                let mkfs = match partition.role {
                    PartitionRole::Esp => {
                        Invocation::new("mkfs.fat", ["-F32", "-n", "ESP", &partition.device])
                    }
                    PartitionRole::Root => {
                        Invocation::new("mkfs.btrfs", ["-f", "-L", "root", &partition.device])
                    }
                    PartitionRole::Other => {
                        Invocation::new("mkfs.btrfs", ["-f", &partition.device])
                    }
                };
                actions.push(Action::Run(mkfs));
                if partition.role == PartitionRole::Root {
                    actions.push(Action::Run(Invocation::new("mkdir", ["-p", TARGET])));
                    actions.push(Action::Run(Invocation::new(
                        "mount",
                        [partition.device.as_str(), TARGET],
                    )));
                    actions.push(Action::Run(Invocation::new(
                        "btrfs",
                        ["subvolume", "create", &format!("{TARGET}/@")],
                    )));
                    actions.push(Action::Run(Invocation::new(
                        "btrfs",
                        ["subvolume", "create", &format!("{TARGET}/@home")],
                    )));
                    actions.push(Action::Run(Invocation::new("umount", [TARGET])));
                }
            }
            Ok(actions)
        }
    }
}

fn step4_mount(plan: &InstallPlan, layout: &DiskLayout) -> Result<Vec<Action>, PipelineError> {
    match plan.disk.mode {
        DiskMode::Erase => Ok(vec![
            Action::Run(Invocation::new(
                "mount",
                ["-o", "subvol=@,compress=zstd", &layout.root_device, TARGET],
            )),
            Action::Run(Invocation::new("mkdir", ["-p", &format!("{TARGET}/home")])),
            Action::Run(Invocation::new(
                "mount",
                [
                    "-o",
                    "subvol=@home,compress=zstd",
                    &layout.root_device,
                    &format!("{TARGET}/home"),
                ],
            )),
            Action::Run(Invocation::new("mkdir", ["-p", &format!("{TARGET}/boot")])),
            Action::Run(Invocation::new(
                "mount",
                [layout.esp_device.as_str(), &format!("{TARGET}/boot")],
            )),
        ]),
        DiskMode::Manual => {
            let partitions = manual_partitions(plan, 4)?;
            // Root first, then everything else, so parent mount points exist.
            let mut ordered: Vec<_> = partitions.iter().collect();
            ordered.sort_by_key(|p| p.mount_point != "/");
            let mut actions = Vec::new();
            for partition in ordered {
                let target_path = format!("{TARGET}{}", partition.mount_point);
                actions.push(Action::Run(Invocation::new("mkdir", ["-p", &target_path])));
                actions.push(Action::Run(Invocation::new(
                    "mount",
                    [partition.device.as_str(), target_path.as_str()],
                )));
            }
            Ok(actions)
        }
    }
}

fn step7_system_config(
    plan: &InstallPlan,
    layout: &DiskLayout,
    adapter: &dyn Adapter,
) -> Vec<Action> {
    let mut actions = vec![
        Action::Run(
            Invocation::new("genfstab", ["-U", TARGET])
                .with_capture(Capture::AppendStdoutTo(format!("{TARGET}/etc/fstab"))),
        ),
        Action::UncommentLine {
            path: format!("{TARGET}/etc/locale.gen"),
            pattern: format!("{} UTF-8", plan.locale),
        },
        Action::Run(arch_chroot(layout, ["locale-gen"])),
        Action::WriteFile {
            path: format!("{TARGET}/etc/locale.conf"),
            content: format!("LANG={}\n", plan.locale),
        },
        Action::WriteFile {
            path: format!("{TARGET}/etc/vconsole.conf"),
            content: format!("KEYMAP={}\n", plan.keyboard.layout),
        },
        Action::Symlink {
            root: TARGET.to_string(),
            link: "/etc/localtime".to_string(),
            points_to: format!("/usr/share/zoneinfo/{}", plan.timezone),
        },
        Action::WriteFile {
            path: format!("{TARGET}/etc/hostname"),
            content: format!("{}\n", plan.hostname),
        },
        Action::WriteFile {
            path: format!("{TARGET}/etc/systemd/zram-generator.conf"),
            content: "[zram0]\nzram-size = ram / 2\ncompression-algorithm = zstd\n".to_string(),
        },
    ];

    actions.extend(adapter.keyboard_config(plan, layout));

    if let Some(profile) = &plan.network_profile {
        // NetworkManager ignores a keyfile anyone but root can read, and
        // the directory only exists if a package created it.
        actions.push(Action::Run(Invocation::new(
            "install",
            [
                "-D".to_string(),
                "-m".to_string(),
                "600".to_string(),
                format!("/etc/NetworkManager/system-connections/{profile}.nmconnection"),
                format!("{TARGET}/etc/NetworkManager/system-connections/{profile}.nmconnection"),
            ],
        )));
    }

    actions
}

fn step8_user(plan: &InstallPlan, layout: &DiskLayout, adapter: &dyn Adapter) -> Vec<Action> {
    let group = adapter.admin_group();
    vec![
        Action::Run(arch_chroot(
            layout,
            [
                "useradd",
                "-m",
                "-G",
                group,
                "-c",
                &plan.user.full_name,
                "-s",
                "/bin/bash",
                &plan.user.username,
            ],
        )),
        Action::Run(
            arch_chroot(layout, ["chpasswd"]).with_stdin(Stdin::Redacted(format!(
                "{}:{}",
                plan.user.username,
                plan.user.password.expose()
            ))),
        ),
        Action::Run(arch_chroot(layout, ["passwd", "-l", "root"])),
        Action::WriteFile {
            path: format!("{TARGET}/etc/sudoers.d/10-{group}"),
            content: format!("%{group} ALL=(ALL:ALL) ALL\n"),
        },
    ]
}

fn step10_secure_boot(layout: &DiskLayout) -> Vec<Action> {
    vec![
        Action::Run(arch_chroot(layout, ["sbctl", "create-keys"])),
        Action::Run(arch_chroot(
            layout,
            [
                "sbctl",
                "sign",
                "-s",
                "/boot/EFI/systemd/systemd-bootx64.efi",
            ],
        )),
        Action::Run(arch_chroot(
            layout,
            ["sbctl", "sign", "-s", "/boot/EFI/Linux/arch-linux.efi"],
        )),
        Action::Run(arch_chroot(
            layout,
            [
                "sbctl",
                "sign",
                "-s",
                "/boot/EFI/Linux/arch-linux-fallback.efi",
            ],
        )),
        Action::Run(arch_chroot(layout, ["sbctl", "enroll-keys", "--microsoft"])),
    ]
}

fn step12_finish(plan: &InstallPlan, layout: &DiskLayout, adapter: &dyn Adapter) -> Vec<Action> {
    let mut actions = adapter.services(plan, layout);
    actions.push(Action::Run(Invocation::new(
        "cp",
        [LOG_FILE, &format!("{TARGET}{LOG_FILE}")],
    )));
    actions.push(Action::Run(Invocation::new("umount", ["-R", TARGET])));
    actions.push(Action::Run(Invocation::new("sync", Vec::<String>::new())));
    actions
}
