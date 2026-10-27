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

## Copied-install discovery

Run the inventory command against a disposable copied root:

```sh
cargo run -p ajour-core --bin discover_wow --locked \
  --features no-self-update -- <copied-WoW-root>
```

It prints YAML without loading application configuration, contacting
providers or creating directories. Use disposable copies for development. Exit
status 1 means the report contains an unresolved product or addon; status 2
means the command arguments are invalid. Invalid root metadata is an error.

Discovery reads active `.build.info` records and keeps modern product identities
separate from the legacy `Flavor` configuration. Unknown products, future version
families and duplicate active records require resolution. Existing configuration
is preserved, including old TBC selections. The source-install command now passes
its selected legacy flavor to repository metadata construction.

Observed Blizzard US version-service responses on 2026-10-27 were:

| Product | Version | Expected directory |
| --- | --- | --- |
| `wow` | `12.1.5.70575` | `_retail_` |
| `wow_classic` | `5.5.4.70579` | `_classic_` |
| `wow_classic_era` | `1.15.9.70576` | `_classic_era_` |
| `wow_anniversary` | `2.5.6.70577` | `_anniversary_` |

The service endpoints use
`https://us.version.battle.net/v2/products/<product>/versions`.
These observations establish product/version families; they do not establish
which build a user has installed. Directory conventions follow the current
[WowUp constants](https://github.com/WowUp/WowUp/blob/master/wowup-electron/src/common/constants.ts),
and still need native installation verification. The build-info parser follows
the typed fields used by
[CascLib](https://github.com/ladislav-zezula/CascLib/blob/master/src/CascFiles.cpp).
TOC suffixes and interface-number calculation follow
[BigWigs packager](https://github.com/BigWigsMods/packager/blob/master/release.sh).

The report selects a TOC candidate only when exactly one recognized filename
declares the exact client interface. It does not reproduce the full game loader:
older interfaces, qualified Interface headers and unrecognized suffixes remain
unresolved. A selected candidate does not certify addon compatibility or permit
installation. PTR and other unlisted products are unsupported. There is no GUI
integration or configuration migration in this milestone.

Reads are bounded to 1 MiB per metadata file, 64 MiB of TOCs per discovery,
10,000 build rows or addon entries per product, and 64 build-info columns.
Links and Windows reparse points are rejected when inspected. Discovery is a
read-only snapshot; concurrent filesystem changes can invalidate it. Never use
this report as authorization for later filesystem writes without revalidation.
Offline fixtures cover copied Windows, macOS and Wine/Linux layouts, malformed
and ambiguous metadata, read limits, links and unchanged legacy configuration.
Executing those fixtures on Windows does not verify native macOS or Linux support.

## GitHub provider pilot

`check_github <owner/repository> <copied-WoW-root> <product-id>` previews a
stable public release without loading application configuration or installing it.
The pilot requires a unique `release.json` match for the selected modern product
and exact interface, a packaged ZIP, and GitHub SHA-256 digests. Missing or
ambiguous metadata is an error. It does not guess from filenames or fall back
across products, release channels or providers.

Downloads require HTTPS without URL credentials. Redirects stay on the same
origin, with GitHub release downloads also allowed to reach
`release-assets.githubusercontent.com`. There are at most five redirects and a
120-second timeout per download request (30 seconds for metadata). Metadata is
limited to 1 MiB and archives to 128 MiB. HTTP errors, encoded bodies, incomplete
streams and size or digest mismatches fail before publication. The downloader
uses an owned temporary file and never replaces an existing download. A forced
process stop can leave an unpublished temporary file; retry preserves it.
This also hardens the shared legacy addon download path. Unverified providers
with other redirect conventions may now report an error and need contract review.

The library pilot can update a disposable copied installation through the staged
installer. It rechecks installation identity after downloading and validates all
staged addon TOCs before replacing folders. Returned metadata comes from the
selected product TOC. Callers must explicitly supply previously owned folders;
discovery does not establish repository ownership. The returned receipt is in
memory, and `matches_release` compares that receipt with remote metadata. It
does not verify later changes on disk. Persistent ownership, GUI integration and
the recovery cleanup flow are still required before public beta.

The SHA-256 implementation uses `sha2` 0.10.9 and seven new locked crypto
packages including `sha2`. Its CPU-detection dependency requires a newer `libc`,
which is pinned to 0.2.155, up from 0.2.98. This meets the CPU-detection
minimum and retains the deprecated Linux `ENOATTR` alias used by locked
`xattr` 0.2.2; newer `libc` removed that alias and failed Linux compilation.
Other existing dependency versions and the
lockfile format are preserved. The already locked `tempfile` package is now a
runtime dependency as well as the source of disposable test fixtures.

On 2026-10-27, an explicit unauthenticated probe through the Rust adapter checked
the public BigWigs v427.8 metadata and manifest, then verified its 4,635,969-byte
ZIP against the API digest. The ZIP was only read, never installed or executed.
This establishes a download contract for that observed release, not compatibility
or coverage of every repository. The probe is ignored during normal tests and
requires `PATCHLOOM_PUBLIC_GITHUB_PROBE=1` when explicitly invoked.
GitHub documents public unauthenticated reads with a 60-request/hour primary
limit per originating IP. The adapter makes no automatic retries on HTTP errors;
callers must respect rate-limit responses before requesting again. See GitHub's
[rate-limit documentation](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)
and the [packager manifest source](https://github.com/BigWigsMods/packager/blob/master/release.sh).

Offline fixtures cover the selected-product update/recheck loop, incompatible
and malicious ZIPs, HTTP failures, truncation, byte limits, timeouts, redirects,
digest failures, links, destination races, cancellation and forced interruption.
Existing installer rollback and process-interruption fixtures remain in the
suite. These checks use disposable data; they do not certify live updates,
power-loss durability, hostile concurrent filesystem writers or native macOS
behavior. The old Ajour application self-update helper remains disabled through
`no-self-update` and is outside this addon-download pilot.
