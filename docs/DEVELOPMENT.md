# Patchloom developer notes

## Restoring a retained original bundle

The standalone `restore_originals` command previews the original bundle and
affected current folders while holding the existing installer lock. Type
`RESTORE` to select the originals; any other response cancels without changing
addon folders. Use only disposable fixtures during development.

```sh
cargo run -p ajour-core --bin restore_originals --locked \
  --features no-self-update -- <fixture-AddOns-directory>
```

Recovery requires a valid interrupted-install journal and every original backup.
Current folders move into `.patchloom-restore/backup` before originals return to
AddOns. Replacement versions and both journals remain intact. Interrupted
replacement retention can resume after confirming a fresh preview. If any
original has already moved back, recovery stops for inspection instead of
guessing that an occupied destination is the original. Automatic resume after
partial original restoration needs additional source-provenance validation.
Conflicts, missing originals, links and busy locks stop the operation.

Updates stay blocked until explicit cleanup is implemented and reviewed. This
command does not choose the replacement version, recover an incomplete recovery
preparation, or certify power-loss durability. It never starts the GUI or reads
application configuration. A coherent hostile writer remains outside the
cooperating-process guarantee.

## Inspecting an interrupted install

The standalone `inspect_recovery` command reads a supplied AddOns fixture and
prints the journal phase plus retained backup, staging and destination folders.
It does not start the GUI, load application configuration, contact providers,
or create, restore or delete addon/journal files. If an installer lock file
exists, its lock must be available; busy or malformed state returns an error.

```sh
cargo run -p ajour-core --bin inspect_recovery --locked \
  --features no-self-update -- <fixture-AddOns-directory>
```

The report is an inspection snapshot. Journal claims and marker names do not
authorize recovery actions. Keep ambiguous versions intact; selecting and
executing a verified restore remains a later milestone. Missing lock files
and non-cooperating writers limit snapshot consistency. Use disposable fixtures
while this workflow is under development.

## Development status

There is no public Patchloom release yet. The current work is on
[`patchloom/safe-install-core`](https://github.com/jacpub/Patchloom/tree/patchloom/safe-install-core).
Provider integrations, current WoW flavor support and application migration
still need work before everyday use.

The installer now validates and stages archives before replacing addon folders,
keeps originals for rollback, and preserves ambiguous interrupted updates for
explicit recovery. Recovery UI and further platform checks remain unfinished.
Use disposable fixtures for development; do not run these builds against a live
WoW installation or your normal application configuration.

| Target | Validation completed so far |
| --- | --- |
| Windows x64 | 37 local tests, including four process-interruption tests |
| Linux x64 | Ubuntu 24.04 CI builds and 33 tests per configuration |
| macOS | Native build, runtime and packaging validation pending |

Windows and Linux checks use Rust 1.99.1 with self-update disabled. One catalog
network test is excluded. Linux does not run the Windows-only subprocess tests.
Strict Clippy still reports existing code issues, so overall CI is not fully
passing. These results do not certify released packages or live addon updates.
See the [installer development notes](SAFE_INSTALL_DEVELOPMENT.md) and
[CI workflow](../.github/workflows/continuous-integration.yml) for details.

## Building and testing

Use Rust 1.99.1 for the current development baseline. Windows builds need MSVC
v143 and a compatible Windows SDK. Linux CI uses Ubuntu 24.04 with FreeType,
Fontconfig and XKB development libraries; the workflow lists its native
prerequisites. The core crate declares Rust 1.89 as its minimum, but that
compiler has not yet been tested.

Clone the current development branch:

```sh
git clone -b patchloom/safe-install-core https://github.com/jacpub/Patchloom.git
cd Patchloom
```

Before running checks, isolate application data. In a Linux shell:

```sh
export XDG_CONFIG_HOME="$(mktemp -d)"
```

In Windows PowerShell:

```powershell
$env:APPDATA = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid())
```

Then run the development checks:

```sh
cargo check --workspace --locked --features no-self-update
cargo test --workspace --locked -F no-self-update -- --skip=test_catalog_download
cargo fmt --all -- --check
```

Package and executable names still use `ajour`. Keep `no-self-update` enabled:
default builds still look for Ajour application releases. Legacy self-update
helpers and configuration migration need separate work before executable trials.
The old [installation guide](../INSTALL.md) and third-party Ajour packages describe
upstream Ajour, not a Patchloom release.

## Contributing and community

Use [Discussions](https://github.com/jacpub/Patchloom/discussions) for questions
and ideas. Report reproducible bugs in
[Issues](https://github.com/jacpub/Patchloom/issues), including your OS, commit,
steps and relevant logs. Remove account names, tokens and private paths from
logs.

Keep pull requests focused and say which platforms and checks you ran.
Filesystem changes should include disposable fixture tests that preserve
unrelated addons and saved settings. See the
[pull request template](../.github/PULL_REQUEST_TEMPLATE.md) and
[changelog](../CHANGELOG.md). Provider access and download rules must be verified
before enabling an integration.

Follow the [brand and public-copy guidance](BRAND_DIRECTION.md) for all
Patchloom text and visuals.
