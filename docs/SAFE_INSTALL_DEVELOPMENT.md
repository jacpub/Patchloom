# Safe installer development status

Patchloom revives Ajour under its original GPLv3 license. Existing contributor
and artwork attribution remains intact. This change replaces the shared addon
installation path; application identity, providers and GUI are unchanged.

## Behavior

- Validate the complete ZIP before changing installed folders. Reject unsafe
  paths, links, collisions, unsupported layouts and resource-limit excess.
- Stage on the destination volume, retain originals in a backup, and serialize
  cooperating installers with an OS-held lock. Rust 1.89 is the declared minimum
  for standard-library file locking; validation used Rust 1.99.1 on Windows x64.
- Roll back failed publication using the in-memory list of published folders.
  Restart recovery restores originals into empty destinations or finishes valid
  committed cleanup. Ambiguous publication retains both versions and blocks for
  explicit recovery; recovery UI is still pending.

This does not guarantee an atomic view across multiple addon folders, power-loss
durability, or protection against hostile concurrent same-user writers. Normal
inventory does not automatically recover an interrupted installation.

## Validation and review

On 2026-10-26, the Windows offline workspace passed 37 tests: 14 original tests,
19 installer fixtures and four subprocess tests. One protocol helper is
ignored by the parent suite and explicitly executed in child processes. One
catalog network test is excluded. Workspace check and formatting passed.

The process tests perform nine forced child stops, real cross-process lock
contention/release and a final commit-marker rename failure. They use guarded
disposable temporary roots and are compiled only for Windows tests with
`no-self-update`. Process termination is not a power-loss simulation.

Independent review covered the installer logic and reran its 19 fixture tests
after two conditional journal-trust findings were fixed. The later subprocess
harness was author-tested; it has not received that independent review.

Strict Clippy remains a failing check. The latest Windows all-target run reports
65 core-library findings, 70 core-library-test findings and three widget
findings per target in existing code. No diagnostic referenced the installer or
process-test module. These counts are platform/toolchain-specific, not an
allowlist. No lint is suppressed and no failing job is allowed to pass.

Use a disposable application-data directory and build target for local checks.
Never point tests or developer executables at a live WoW installation or
ordinary application configuration. Current validation commands are:

```sh
cargo test --workspace --locked --features no-self-update -- \
  --skip catalog::tests::test_catalog_download
cargo check --workspace --locked --features no-self-update
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked \
  --features no-self-update -- -D warnings
```

## Development CI

Push/PR CI performs checks only: the automatic POEditor upload was removed.
Cargo check/test/Clippy use `--locked` and `no-self-update`. Tests exclude the
catalog network contract check. Dependency/toolchain downloads and GitHub
action/cache traffic still occur. CI configuration uses a
temporary Linux application-data root; existing jobs run on Linux and do not
execute the Windows-only process tests.

Manual release, crates publication and translation-import workflows remain
unchanged and must not be dispatched during revival. Development CI uploads no
packages and creates no tags or releases. Ubuntu 24.04 CI with Rust 1.99.1 and
explicit native font/XKB prerequisites now passes both build/test
configurations:
33 tests per configuration, with one catalog network test excluded. Linux does
not run the Windows-only subprocess tests. Strict lint still fails on existing
widget findings; no lint suppression was added.

## Before live trials or distribution

Complete explicit recovery inspection/restore/cleanup UX; native macOS/Linux
locking and recovery checks; Rust 1.89 verification; provider authorization,
download validation and compatible modern WoW flavor fixtures. Resolve old
package/config identity and migration through an explicit copied import.

Default builds still discover Ajour application releases. The `no-self-update`
feature disables that lookup, but leaves legacy helpers and the cleanup CLI
path compiled. Hard-disable or redesign those paths before launching or
distributing revival builds. Renaming a GitHub repository does not fix runtime
identity. No live WoW trial or binary release is certified by these results.
