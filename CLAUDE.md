# CLAUDE.md — Dawn (LuminOS installer)

Dawn is the graphical installer for LuminOS: a Rust + Slint frontend, a privileged Rust backend, and an InstallPlan JSON between them. `SPEC.md` is the source of truth. Read it before starting any milestone, and re-read the sections a task touches.

## How we work

- Work the milestones in `SPEC.md` strictly in order, M0 → M7. One milestone at a time.
- One branch per milestone, named `m<N>-<short-name>` (e.g. `m0-workspace`), created from an up-to-date `main`.
- Small, logical commits with clear messages.
- When a milestone is done, open a pull request and **stop**. Don't start the next milestone until the user has reviewed and merged it.
- Never merge, never push to `main`, never force-push a shared branch.
- If `gh` is unavailable, push the branch and put the PR text in your reply instead.

Every PR description contains:

1. What was built, in a few lines.
2. The milestone's Done-when check: the exact command run and its result.
3. Anything that could not be verified locally, and why.
4. New entries added to `DECISIONS.md`.
5. Open questions for the user.

## When to stop and ask

- The spec and the code disagree, or the spec doesn't cover a choice that matters. Ask, then record the answer in `DECISIONS.md`.
- A task needs root on the host machine.
- A check needs a VM and none is available (see below).
- A dependency's licence isn't clearly GPLv3-compatible.

## VM and test environment

Where VM installs run is not decided yet.

- Before any step that needs a VM, check for `/dev/kvm`, `qemu-system-x86_64` and OVMF firmware.
- If any is missing, don't improvise: run what works without a VM (unit tests, dry-run snapshots, the UI in mock-backend mode) and report the gap in the PR.
- M1's Done-when check needs a VM. Before starting M1, ask the user whether VM tests run locally or in CI.

## Safety rules (non-negotiable)

- Never run anything as root on the host. Privileged work (loop devices, partitioning, chroot, pacstrap) runs only inside a disposable VM.
- Never run the backend without `--dry-run` outside such a VM.
- Never reference `/dev/sd*` or `/dev/nvme*` in code paths under test; use loop devices and VM disks only.
- Never run `sbctl enroll-keys` outside a VM with throwaway OVMF variables.
- Never point tests at public Arch or LuminOS mirrors; use the pinned local mirror.
- The backend refuses to run unless `--target` matches the plan's disk exactly, and refuses any disk that is mounted or holds the running system. Don't weaken these checks, even temporarily.

## Code conventions

- Rust stable, edition 2024, one Cargo workspace: `plan`, `backend`, `frontend` (layout in `SPEC.md`).
- Every external command goes through the single runner module, built as an argument list, never a shell string. That's where dry-run, logging and redaction live.
- No `unwrap()` or `expect()` in backend code outside tests; errors carry the pipeline step they came from.
- The user password never appears in logs, errors, snapshots or test fixtures.
- Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- UI strings go through `@tr(...)`. Branding values come from the `Theme` global, never hard-coded.

## Commands

These exist once M0 lands; keep this list current as tooling is added.

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo run -p backend -- --dry-run tests/plans/erase-online.json
```

All five must pass before a PR is opened.
