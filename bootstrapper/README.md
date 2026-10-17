# Setup stub

A small Windows program (`<Shortname>Setup.exe`, under 5 MB, no runtime needed) that downloads the
desktop client's MSI, checks it, installs it and opens the app, the way browser installers work.

## What it does

1. Reads the machine:
   - Windows build and architecture;
   - the installed version of this product, found by its MSI upgrade code;
   - whether the stock upstream client is installed;
   - whether another installation is running.
2. Downloads `latest.json` and `latest.json.sig` from the update host. The host is fixed at build time from `oem.json`, and the download uses HTTPS only, with no redirects.
3. Verifies the minisign signature with the public key compiled in from `oem.json` (`bootstrapper.manifest_pubkey`). Only pre-hashed `ED` signatures are accepted, and the trusted comment is checked too.
4. Checks the manifest:
   - product name and channel;
   - the URL shape, `https://<host>/client/<name>.msi`;
   - the size and the SHA-256;
   - the version is not older than the version this stub was built with. This blocks a replayed old manifest.
5. Decides what to do:
   - Windows too old, or the architecture is not supported: it stops with a message;
   - the stock client is installed: it asks for it to be removed first, and offers a button that opens Installed apps;
   - the same or a newer version is installed: it just opens it and never downgrades;
   - otherwise: it installs.
6. Downloads the MSI into a new folder under `%TEMP%`:
   - resume after an interruption, with 3 retries;
   - never more bytes than the manifest says;
   - the SHA-256 must match, or nothing is run.
7. Runs `msiexec /i <msi> /qb! /norestart /l*v <log>` elevated. The stub itself runs as the user, so the Windows permission (UAC) prompt appears once, for the installer only.
8. Deletes the download, opens the app and exits.

## Silent mode

`<Shortname>Setup.exe /S [/log <file>]` runs with no window. The exit code is msiexec's own, or one of these:

| Code | Meaning |
|---|---|
| 0 | Installed, or already current |
| 3010 | Installed; restart needed |
| 1602 | Cancelled |
| 1612 | Update server not reachable |
| 1618 | Another installation is running |
| 1330 | Manifest or installer failed the checks |
| 1633 | Windows version or architecture not supported |
| 1638 | The stock client must be removed first |
| 112 | Disk full |
| 1603 | Other failure |

Run it with `start /wait` so the exit code is kept.

## Build

```
cd bootstrapper
cargo test --locked --target x86_64-pc-windows-msvc
cargo build --release --locked --target x86_64-pc-windows-msvc
```

- **Toolchain:** pinned in `rust-toolchain.toml`.
- **Dependencies:** pinned in `Cargo.lock`.
- **CI:** `.github/workflows/setup-stub.yml` builds twice, fails if the two builds differ, and publishes the SHA-256.
- **Without a manifest key** (`SDRIVE_DEV_BUILD=1`), the stub builds as a test build that refuses to install.

## Code layout

- **Portable logic, unit-tested on any OS:** `src/app.rs` (the whole run), `manifest.rs`, `minisign.rs`, `plan.rs`, `download.rs` (resume, retry and hash, against a local test server), `exitcode.rs`, `strings.rs` (every text, in one table), `args.rs`, `version.rs`.
- **Windows only:** `src/win/` (WinHTTP, registry, MSI, the window).
