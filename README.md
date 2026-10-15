# Shaavir Drive desktop client - branding and build files

This repository holds the branding ("OEM theme") and build files for the **Shaavir Drive** desktop
client for Windows, published by Shaavir. Shaavir Drive is the open-source Nextcloud desktop client,
built with the theme, build scripts and the small source patch in this repository. The exact build-time
changes to the upstream source are listed below; there are no others.

## Source code (GNU GPL v2 or later)

The Shaavir Drive application is a build of the Nextcloud desktop client, which is licensed under
the GNU General Public License, version 2 or later. The complete corresponding source code for every
binary Shaavir distributes is:

| Part | Where | Exact revision |
|---|---|---|
| Application source | https://github.com/nextcloud/desktop.git | tag `v34.0.5`, commit `62ebad6043b1e7c8e319f41be25d41f5a2c733e5` |
| Branding, build scripts, installer artwork | this repository (https://github.com/SHAAVIR-CONSULTING-MGMT-SERVICES/shaavir-drive-client) | tag `client-v34.0.5-1` |
| Build tool (KDE Craft, Nextcloud fork) | https://github.com/nextcloud/craft | branch `stable-34.0-qt-6.10.2`, commit `2661a4e055f1e077548adf3ba050a4699837e574` |
| Build recipes | https://github.com/nextcloud/craft-blueprints-nextcloud, https://github.com/nextcloud/craft-blueprints-kde | commits `be49d8769cc45f704aa0813bb01b4ef138444485`, `f741913be9544ee325beced18c8f357fc3f439bc` |
| Code patches | `patches/` in this repository | `0001-enforced-server-url-ignores-overrides.patch` (see below) |

The third-party libraries in the installer are built by the Craft recipes above from their own
published sources.

**Written offer.** For at least three years after the last distribution of any Shaavir Drive binary,
Shaavir will give **any third party**, on request to support@shaavir.com, a complete machine-readable copy
of the corresponding source code (everything in the table above) for no more than the cost of physically
performing the distribution.

### Changes made to the upstream source at build time

| Change | Made by | Why |
|---|---|---|
| One line appended to `NEXTCLOUD.cmake`: `include("${CMAKE_SOURCE_DIR}/OEM.cmake")`, plus `OEM.cmake` copied next to it | `scripts/apply-oem.sh` | Product name, server, update feed, behaviour options and Windows GUIDs (values in `OEM.cmake`) |
| Files under `theme/` and `admin/win/msi/gui/` replaced by the files in `theme/` and `installer/windows/` here | `scripts/apply-oem.sh` | Icons, logos and installer artwork. Upstream's code loads the stock logo by the fixed file name `wizard-nextcloud.*` (`src/libsync/theme.cpp`); those files keep that name but contain this product's logo |
| `patches/0001-enforced-server-url-ignores-overrides.patch` applied | `scripts/apply-oem.sh` | With the server URL enforced, a saved `--overrideserverurl` is ignored and command-line account setup (`--serverurl`) refuses any other server |
| The Craft recipe `nextcloud-client` (build tool input, not client source): kept-executable list and package names changed to this product's executable | `scripts/patch-blueprint.py` | Otherwise the packaging step would drop the branded executable |

License: see `LICENSE`. This repository's own files are distributed under the same terms
(GPL-2.0-or-later).

"Nextcloud" is a trademark of Nextcloud GmbH. Shaavir Drive is not a Nextcloud product and is not
endorsed by Nextcloud GmbH; no Nextcloud name or logo is used in its branding.

## This release

| Item | Value |
|---|---|
| Release tag | `client-v34.0.5-1` |
| Client version (shown in About, used by the updater) | `34.0.5.1` |
| Installer | `ShaavirDrive-34.0.5.1-x64.msi` (Windows x64, MSI) |
| Server | https://drive.shaavir.com |
| Update feed | https://updates.shaavir.com/client/ |
| Download page | https://www.shaavir.com/drive/download |

Every installer is published with a `SHA256SUMS` file. Check it before installing:

```powershell
Get-FileHash .\ShaavirDrive-34.0.5.1-x64.msi -Algorithm SHA256
```

## What is in this repository

| Path | What |
|---|---|
| `OEM.cmake` | Branding and behaviour settings, appended to upstream `NEXTCLOUD.cmake` at build time |
| `oem.json` | The same values in machine-readable form, plus the pinned upstream revisions |
| `theme/` | Icon and logo overrides, same file names as upstream `theme/` |
| `installer/windows/` | MSI dialog artwork (`banner.bmp` 493x58, `dialog.bmp` 493x314) |
| `defaults/` | Optional Windows registry defaults and policies for administrators |
| `scripts/apply-oem.sh` | Copies the theme into an upstream checkout and hooks `OEM.cmake` in |
| `scripts/patch-blueprint.py` | Points the Craft recipe at this product's executable name |
| `scripts/check-branding.sh` | Fails the build if a stock name or logo would ship |
| `scripts/gen-icons.sh` | Re-renders the PNG artwork from the SVGs (Inkscape or rsvg-convert) |
| `.github/workflows/windows-msi.yml` | The build: unsigned MSI plus `SHA256SUMS` on a GitHub Windows runner |
| `patches/` | Source patches (empty unless a change to upstream code is unavoidable) |

## Building

The workflow `.github/workflows/windows-msi.yml` is the reference build. It runs on a
`windows-2022` GitHub runner (Visual Studio 2022, WiX Toolset 3.14, Python 3.12) and:

1. checks out this repository at the release tag and the upstream client at `v34.0.5`,
   refusing to build if the tag no longer points to `62ebad6043b1e7c8e319f41be25d41f5a2c733e5`;
2. runs `scripts/check-branding.sh`;
3. sets up KDE Craft and the pinned recipes exactly as upstream's own Windows CI does at that tag,
   then applies the theme (`scripts/apply-oem.sh`) and the recipe change (`scripts/patch-blueprint.py`);
4. builds with `-DBUILD_WIN_MSI=ON -DMIRALL_VERSION_BUILD=<n>`, collects the application files
   (Craft portable package) and builds the MSI with upstream's `make-msi.bat`;
5. writes `SHA256SUMS` and uploads the MSI, the checksums and a build record as workflow artifacts.

The first build downloads and compiles the Craft dependencies and can take several hours; later builds
reuse the cache.

To build by hand on Windows 10/11, follow the same steps from the workflow file in a
"x64 Native Tools Command Prompt for VS 2022" with Python 3.12, Git, Inkscape and the WiX Toolset 3.14
installed; upstream's general instructions are in the craft-blueprints-nextcloud README.

## Behaviour of this build

- Server address: https://drive.shaavir.com.
- Windows virtual files (placeholders that download on open) are always on.
- One account per installation; accounts from a stock client are never imported.
- With the server enforced, a saved `--overrideserverurl` is ignored and command-line account setup refuses
  any other server (`patches/0001`).
- No crash reporter; no provider sign-up list.
- Updates are checked against https://updates.shaavir.com/client/.

## Reporting a problem

Contact Shaavir through https://www.shaavir.com/drive/download.
