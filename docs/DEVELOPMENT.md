# Patchloom developer notes

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
