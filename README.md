![banner](.github/assets/pixi-pack-banner-dark.svg#gh-dark-mode-only)
![banner](.github/assets/pixi-pack-banner-light.svg#gh-light-mode-only)

<div align="center">

[![License][license-badge]](LICENSE)
[![CI Status][ci-badge]][ci]
[![Conda Platform][conda-badge]][conda-url]
[![Conda Downloads][conda-downloads-badge]][conda-url]
[![Project Chat][chat-badge]][chat-url]
[![Pixi Badge][pixi-badge]][pixi-url]

[license-badge]: https://img.shields.io/github/license/quantco/pixi-pack?style=flat-square
[ci-badge]: https://img.shields.io/github/actions/workflow/status/quantco/pixi-pack/ci.yml?style=flat-square&branch=main
[ci]: https://github.com/quantco/pixi-pack/actions/
[conda-badge]: https://img.shields.io/conda/vn/conda-forge/pixi-pack?style=flat-square
[conda-downloads-badge]: https://img.shields.io/conda/dn/conda-forge/pixi-pack?style=flat-square
[conda-url]: https://prefix.dev/channels/conda-forge/packages/pixi-pack
[chat-badge]: https://img.shields.io/discord/1082332781146800168.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2&style=flat-square
[chat-url]: https://discord.gg/kKV8ZxyzY4
[pixi-badge]: https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/prefix-dev/pixi/main/assets/badge/v0.json&style=flat-square
[pixi-url]: https://pixi.sh

</div>

![pixi-pack demo](.github/assets/demo/demo-light.gif#gh-light-mode-only)
![pixi-pack demo](.github/assets/demo/demo-dark.gif#gh-dark-mode-only)

## 🗂 Table of Contents

- [Introduction](#-introduction)
- [Installation](#-installation)
- [Usage](#-usage)

## 📖 Introduction

Starting with a [pixi](https://pixi.sh) lockfile `pixi.lock`, you can create a packed environment that can be shared with others.
This environment can be unpacked on any system using `pixi-unpack` to recreate the original environment.

In contrast to [`conda-pack`](https://conda.github.io/conda-pack/), `pixi-pack` does not require the original conda environment to be present on the system for packing.
Instead, it uses the lockfile to download the required packages and puts them into a `.tar` archive.
This archive can then be shared with others and installed using `pixi-unpack` to recreate the original environment.

The original motivation behind `pixi-pack` was to create a `conda-pack` alternative that does not have the same reproducibility issues as `conda-pack`.
It also aims to allow cross-platform building packs, so you can create a pack for `win-64` on a `linux-64` system.

## 💿 Installation

You can install `pixi-pack` and `pixi-unpack` using `pixi`:

```bash
pixi global install pixi-pack
pixi global install pixi-unpack
```

Or using `cargo`:

```bash
cargo install --locked --git https://github.com/quantco/pixi-pack.git
```

Or by downloading our pre-built binaries from the [releases page](https://github.com/quantco/pixi-pack/releases).

Instead of installing `pixi-pack` and `pixi-unpack` globally, you can also use `pixi exec` to run `pixi-pack` in a temporary environment:

```bash
pixi exec pixi-pack
pixi exec pixi-unpack environment.tar
```

> [!NOTE]
> You can also write `pixi pack` (and `pixi unpack`) if you have `pixi-pack` and `pixi-unpack` installed globally.

## 🎯 Usage

### `pixi-pack`: Packing an environment

With `pixi-pack`, you can pack a conda environment into a `environment.tar` file:

```bash
pixi-pack --environment prod --platform linux-64 pixi.toml
```

This will create an `environment.tar` file that contains all conda packages required to create the environment.

```
# environment.tar
| pixi-pack.json
| environment.yml
| channel
|    ├── noarch
|    |    ├── tzdata-2024a-h0c530f3_0.conda
|    |    ├── ...
|    |    └── repodata.json
|    └── linux-64
|         ├── ca-certificates-2024.2.2-hbcca054_0.conda
|         ├── ...
|         └── repodata.json
```

### `pixi-unpack`: Unpacking an environment

With `pixi-unpack environment.tar`, you can unpack the environment on your target system.
This will create a new conda environment in `./env` that contains all packages specified in your `pixi.toml`.
It also creates an `activate.sh` (or `activate.bat` on Windows) file that lets you activate the environment
without needing to have `conda` or `micromamba` installed.

```bash
$ pixi-unpack environment.tar
$ ls
env/
activate.sh
environment.tar
$ cat activate.sh
export PATH="/home/user/project/env/bin:..."
export CONDA_PREFIX="/home/user/project/env"
. "/home/user/project/env/etc/conda/activate.d/activate_custom_package.sh"
```

### Cross-platform packs

Since `pixi-pack` just downloads the `.conda` and `.tar.bz2` files from the conda repositories, you can trivially create packs for different platforms.

```bash
pixi-pack --platform win-64
```

> [!NOTE]
> You can only `pixi-unpack` a pack on a system that has the same platform as the pack was created for.

### Named platforms

If your `pixi.toml` defines named platforms (requires a lockfile in version 7 or later), you can pack them by name.

```toml
platforms = [{ name = "jetson", platform = "linux-aarch64", cuda = "13" }]
```

```bash
pixi-pack --platform jetson
```

You can still pass the underlying conda platform (e.g. `linux-aarch64`) as long as only one named platform uses it; otherwise `pixi-pack` asks you to use one of the platform names instead.

### Self-extracting binaries

You can create a self-extracting binary that contains the packed environment and a script that unpacks the environment.
This can be useful if you want to distribute the environment to users that don't have `pixi-unpack` installed.

```bash
# unix
$ pixi-pack --create-executable
$ ls
environment.sh
$ ./environment.sh
$ ls
env/
activate.sh
environment.sh
```

```powershell
# windows
PS > pixi-pack --create-executable
PS > ls
environment.ps1
PS > .\environment.ps1
PS > ls
env/
activate.sh
environment.ps1
```

#### Custom pixi-unpack executable path

When creating a self-extracting binary, you can specify a custom path or URL to a `pixi-unpack` executable to avoid downloading it from the [default location](https://github.com/Quantco/pixi-pack/releases/latest).

You can provide one of the following as the `--pixi-unpack-source`:

- a URL to a `pixi-unpack` executable like `https://my.mirror/pixi-pack/pixi-unpack-x86_64-unknown-linux-musl`
- a path to a `pixi-unpack` binary like `./pixi-unpack-x86_64-unknown-linux-musl`

##### Example Usage

Using a URL:

```bash
pixi-pack --create-executable --pixi-unpack-source https://my.mirror/pixi-pack/pixi-unpack-x86_64-unknown-linux-musl
```

Using a path:

```bash
pixi-pack --create-executable --pixi-unpack-source ./pixi-unpack-x86_64-unknown-linux-musl
```

> [!TIP]
> The produced executable is a simple shell script that contains both the `pixi-unpack` binary as well as the packed environment.

### Applications (experimental)

With the `app` cargo feature (`cargo install --features app`, it is not part of the released binaries), `pixi-pack` can turn an environment into an application that users install like any other:

| Platform | Formats                               | Build host                                         |
| -------- | ------------------------------------- | -------------------------------------------------- |
| macOS    | `dmg` (default), `app`                | macOS                                              |
| Windows  | `nsis` (default): an `.exe` installer | Windows, or any host with `makensis` on the `PATH` |
| Linux    | `flatpak` (default)                   | Linux with `flatpak`                               |

Describe the application in a TOML file.
If a package in the environment defines a [menuinst](https://conda.github.io/menuinst/) shortcut (`Menu/*.json`), like `napari` or `spyder`, the application can be based on it.
Its name, description, command, icon and version, the macOS `CFBundleIdentifier` and the Linux `Categories` are used, unless set in the TOML file:

```toml
# app.toml
menu = "napari"
# only needed if the shortcut has no CFBundleIdentifier
identifier = "org.napari.napari"
```

Otherwise, describe the application fully:

```toml
# app.toml
name = "Ocean Viewer"
identifier = "org.example.ocean-viewer"
version = "1.0.0"
description = "Look at ocean data"
# executable in the environment to start, and its arguments
entry-point = "python"
args = ["-m", "ocean_viewer"]
icons = ["icon.png"]
# only the formats for the packed platform are created
formats = ["dmg", "nsis", "flatpak"]
# download the packages when unpacking instead of including them
download-packages = true

[macos]
minimum-system-version = "11.0"
signing-identity = "Developer ID Application: Jane Doe (TEAMID)"

[windows]
# sign with signtool and a certificate from the Windows certificate store ...
certificate-thumbprint = "4E6FE4938621C659B9EA8D5F0CF7E96ACD7BE8A0"
timestamp-url = "http://timestamp.digicert.com"
tsp = true
# ... or with any command, `%1` is replaced by the file to sign
# sign-command = "C:/signing/sign.bat %1"

[flatpak]
# the defaults
runtime = "org.freedesktop.Platform"
runtime-version = "25.08"
finish-args = ["--share=ipc", "--share=network", "--socket=x11", "--socket=wayland", "--device=dri", "--filesystem=home"]
```

```bash
pixi-pack --create-app app.toml --platform osx-arm64
```

On macOS and Windows, and for Flatpaks with `download-packages`, the application contains `pixi-launcher`, the pack and a small launcher config.
On first launch, the environment is unpacked into the user's data directory (`~/Library/Application Support/<identifier>`, `%LOCALAPPDATA%\<identifier>` or `~/.var/app/<flatpak id>/data/<identifier>`); later launches start the entry point right away.
When a new version of the application is opened, its environment replaces the old one.
The Windows uninstaller offers to remove the unpacked environment.
A window shows the progress of the first launch (with `zenity` on Linux, which the Flatpak runtime includes).

With `download-packages = true`, the application only contains the packages' metadata, and the first launch downloads them from their channels, checking their SHA256.
This keeps the application small and quick to build, e.g. for environments with CUDA, whose Windows installers would exceed the 2 GB limit of NSIS.
The first launch then needs internet access, and the channels must be public and still serve the packages.
Packages without an `http(s)` URL, e.g. from a local channel, are still included.
Flatpaks need the network permission for this, which they have by default.

On macOS, the application is signed with `codesign` with `signing-identity`, or ad-hoc without it (see [below](#applications-without-signing)).
It is notarized if `APPLE_KEYCHAIN_PROFILE`, or `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID`, or `APPLE_API_KEY`, `APPLE_API_ISSUER` and `APPLE_API_KEY_PATH` are set.

On Windows, the launcher gets the application's icon (an `.ico`, or one created from the largest `.png`) and version info, so the shortcuts and the Task Manager show the application.
The launcher, the installer and the uninstaller are signed if `certificate-thumbprint` (with `signtool`, only on Windows) or `sign-command` (on any platform, e.g. with `osslsigncode` or a cloud signing service) is set.
The sign command is split on spaces, use a script for commands with spaces in paths.

On Linux, `pixi-unpack` unpacks the environment into the Flatpak in the Flatpak build sandbox, as Flatpaks are always installed at the same path, unless `download-packages` is set.
The Flatpak runs on the [Freedesktop runtime](https://flathub.org/apps/org.freedesktop.Platform), which users install along with the application.
For the build, `pixi-pack` adds [Flathub](https://flathub.org) as a remote of the user's Flatpak installation if missing, and installs the runtime from it.
The sandbox permissions (`finish-args`) let the application open windows, use the GPU and the network, and read and write files in the home directory, as conda GUI toolkits often don't use the file chooser portal.
The identifier becomes the Flatpak application id, with `-` replaced by `_` in all but its last part.
Users install the `.flatpak` by opening it in their software center, or with `flatpak install --user app.flatpak`.
Flatpak is preinstalled on e.g. Fedora and Linux Mint, other distributions [need to install it](https://flathub.org/setup) once.
Without `download-packages`, building needs the size of the unpacked environment plus its compressed copy, plus the packages, as free space in the temporary directory (`TMPDIR`).
On GitHub-hosted runners, large environments need [more free space](https://github.com/actions/runner-images/issues/2840).

`pixi-launcher` and `pixi-unpack` are downloaded from the pixi-pack release, use `--pixi-launcher-source` and `--pixi-unpack-source` to provide different ones.
Set `PIXI_LAUNCHER_NO_UI=1` to make it log to stderr instead of showing dialogs, launches are logged to `launcher.log` in the data directory.
Bundling for macOS and Windows is done with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager).
Of a menuinst shortcut, `precommand`, `working_dir` and `terminal` are not supported.
See [`examples/app`](examples/app) for a complete example.

#### Applications without signing

Signing a macOS application with a Developer ID and notarizing it requires a paid [Apple Developer Program](https://developer.apple.com/programs/) membership, signing a Windows installer with a trusted certificate also costs money.
Without them, `pixi-pack` signs macOS applications ad-hoc, and the applications work, but the first launch of a downloaded application needs one extra step.
Applications copied from a USB drive or a network share, or deployed with a device management system, open without it.

On macOS:

1. Open the `.dmg` and drag the application to the `Applications` folder.
2. Open the application. macOS says that it could not verify the application, click **Done**.
3. Open **System Settings** → **Privacy & Security**. Below **Security**, it says that the application was blocked. Click **Open Anyway** and confirm with your password or Touch ID.

This is only needed once per version of the application.

On Windows, Microsoft Defender SmartScreen shows "Windows protected your PC" when starting the installer.
Click **More info**, then **Run anyway**.

### Inject additional packages

You can inject additional packages into the environment that are not specified in `pixi.lock` by using the `--inject` flag:

```bash
pixi-pack --inject local-package-1.0.0-hbefa133_0.conda pixi.toml
```

This can be particularly useful if you build the project itself and want to include the built package in the environment but still want to use `pixi.lock` from the project.
Before creating the pack, `pixi-pack` will ensure that the injected packages' dependencies and constraints are compatible with the packages in the environment.

### PyPi support

You can also pack PyPi wheel packages into your environment.
`pixi-pack` only supports wheel packages and not source distributions.
If you happen to use source distributions, you can ignore them by using the `--ignore-pypi-non-wheel` flag.
This will skip the bundling of all PyPi source distributions.

The `--inject` option also supports wheels.

```bash
pixi-pack --ignore-pypi-non-wheel --inject my_webserver-0.1.0-py3-none-any.whl
```

> [!WARNING]
> In contrast to injecting from conda packages,
> we cannot verify that injected wheels are compatible with the target environment. Please make sure the packages are compatible.

### Mirror and S3 middleware

You can use mirror middleware by creating a configuration file as described in the [pixi documentation](https://pixi.sh/latest/reference/pixi_configuration/#mirror-configuration) and referencing it using `--config`.

```toml
[mirrors]
"https://conda.anaconda.org/conda-forge" = ["https://my.artifactory/conda-forge"]
```

If you are using [S3 in pixi](https://pixi.sh/latest/deployment/s3/), you can also add the appropriate S3 config in your config file and reference it.

```toml
[s3-options.my-s3-bucket]
endpoint-url = "https://s3.eu-central-1.amazonaws.com"
region = "eu-central-1"
force-path-style = false
```

### Setting maximum number of parallel downloads

```toml
[concurrency]
downloads = 5
```

Use `pixi-pack --config config.toml` to use the custom configuration file.
See [pixi docs](https://pixi.sh/latest/reference/pixi_configuration/#concurrency) for more information.

### Cache downloaded packages

You can cache downloaded packages to speed up subsequent pack operations by using the `--use-cache` flag:

```bash
pixi-pack --use-cache ~/.pixi-pack/cache
```

This will store all downloaded packages in the specified directory and reuse them in future pack operations. The cache follows the same structure as conda channels, organizing packages by platform subdirectories (e.g., linux-64, win-64, etc.).

Using a cache is particularly useful when:

- Creating multiple packs with overlapping dependencies
- Working with large packages that take time to download
- Operating in environments with limited bandwidth
- Running CI/CD pipelines where package caching can significantly improve build times

### Unpacking without `pixi-unpack`

If you don't have `pixi-unpack` available on your target system, you can still install the environment if you have `conda` or `micromamba` available.
Just unarchive the `environment.tar`, then you have a local channel named `pixi-unpack` on your system where all necessary packages are available.
Next to this local channel, you will find an `environment.yml` file that contains the environment specification.
You can then install the environment using `conda` or `micromamba`:

```bash
tar -xvf environment.tar
micromamba create -p ./env --file environment.yml
# or
conda env create -p ./env --file environment.yml
```

> [!NOTE]
> The `environment.yml` and `repodata.json` files are only for this use case, `pixi-unpack` does not use them.

> [!NOTE]
> Both `conda` and `mamba` are always installing pip as a side effect when they install python, see [`conda`'s documentation](https://docs.conda.io/projects/conda/en/25.1.x/user-guide/configuration/settings.html#add-pip-as-python-dependency-add-pip-as-python-dependency).
> This is different from how `pixi` works and can lead to solver errors when using `pixi-pack`'s compatibility mode since `pixi` doesn't include `pip` by default.
> You can fix this issue in two ways:
>
> - Add `pip` to your `pixi.lock` file using `pixi add pip`.
> - Configuring `conda` (or `mamba`) to not install `pip` by default by running `conda config --set add_pip_as_python_dependency false` (or by adding `add_pip_as_python_dependency: False` to your `~/.condarc`)

## Build attestations

Build attestations are uploaded to GitHub Artifact Attestations.
You can verify a binary using the `gh` CLI:

```bash
gh attestation verify --repo Quantco/pixi-pack pixi-pack-<architecture>
```
