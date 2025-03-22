![banner](.github/assets/pixi-pack-banner-dark.svg#gh-dark-mode-only)
![banner](.github/assets/pixi-pack-banner-light.svg#gh-light-mode-only)

<div align="center">

[![License][license-badge]](LICENSE)
[![CI Status][ci-badge]][ci]
[![Conda Platform][conda-badge]][conda-url]
[![Project Chat][chat-badge]][chat-url]
[![Pixi Badge][pixi-badge]][pixi-url]

[license-badge]: https://img.shields.io/github/license/quantco/pixi-pack?style=flat-square
[ci-badge]: https://img.shields.io/github/actions/workflow/status/quantco/pixi-pack/ci.yml?style=flat-square&branch=main
[ci]: https://github.com/quantco/pixi-pack/actions/
[conda-badge]: https://img.shields.io/conda/vn/conda-forge/pixi-pack?style=flat-square
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
This environment can be unpacked on any system using `pixi-pack` to recreate the original environment.

In contrast to [`conda-pack`](https://conda.github.io/conda-pack/), `pixi-pack` does not require the original conda environment to be present on the system for packing.
Instead, it uses the lockfile to download the required packages and puts them into a `.tar` archive.
This archive can then be shared with others and installed using `pixi-pack unpack` to recreate the original environment.

The original motivation behind `pixi-pack` was to create a `conda-pack` alternative that does not have the same reproducibility issues as `conda-pack`.
It also aims to allow cross-platform building packs, so you can create a pack for `win-64` on a `linux-64` system.

## 💿 Installation

You can install `pixi-pack` using `pixi`:

```bash
pixi global install pixi-pack
```

Or using `cargo`:

```bash
cargo install --locked --git https://github.com/quantco/pixi-pack.git
```

Or by downloading our pre-built binaries from the [releases page](https://github.com/quantco/pixi-pack/releases).

Instead of installing `pixi-pack` globally, you can also use `pixi exec` to run `pixi-pack` in a temporary environment:

```bash
pixi exec pixi-pack pack
pixi exec pixi-pack unpack environment.tar
```

## 🎯 Usage

### `pixi-pack pack`: Packing an environment

With `pixi-pack pack`, you can pack a conda environment into a `environment.tar` file:

```bash
pixi-pack pack --manifest-file pixi.toml --environment prod --platform linux-64
```

This will create a `environment.tar` file that contains all conda packages required to create the environment.

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

### `pixi-pack unpack`: Unpacking an environment

With `pixi-pack unpack environment.tar`, you can unpack the environment on your target system.
This will create a new conda environment in `./env` that contains all packages specified in your `pixi.toml`.
It also creates an `activate.sh` (or `activate.bat` on Windows) file that lets you activate the environment
without needing to have `conda` or `micromamba` installed.

```bash
$ pixi-pack unpack environment.tar
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
pixi-pack pack --platform win-64
```

> [!NOTE]
> You can only `unpack` a pack on a system that has the same platform as the pack was created for.

### Self-extracting binaries

You can create a self-extracting binary that contains the packed environment and a script that unpacks the environment.
This can be useful if you want to distribute the environment to users that don't have `pixi-pack` installed.

```bash
# unix
$ pixi-pack pack --create-executable
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
PS > pixi-pack pack --create-executable
PS > ls
environment.ps1
PS > .\environment.ps1
PS > ls
env/
activate.sh
environment.ps1
```

> [!TIP]
> The produced executable is a simple shell script that contains both the `pixi-pack` binary as well as the packed environment.

### Inject additional packages

You can inject additional packages into the environment that are not specified in `pixi.lock` by using the `--inject` flag:

```bash
pixi-pack pack --inject local-package-1.0.0-hbefa133_0.conda --manifest-pack pixi.toml
```

This can be particularly useful if you build the project itself and want to include the built package in the environment but still want to use `pixi.lock` from the project.
Before creating the pack, `pixi-pack` will ensure that the injected packages' dependencies and constraints are compatible with the packages in the environment.

### Cache downloaded packages

You can cache downloaded packages to speed up subsequent pack operations by using the `--use-cache` flag:

```bash
pixi-pack pack --use-cache ~/.pixi-pack/cache
```

This will store all downloaded packages in the specified directory and reuse them in future pack operations. The cache follows the same structure as conda channels, organizing packages by platform subdirectories (e.g., linux-64, win-64, etc.).

Using a cache is particularly useful when:

- Creating multiple packs with overlapping dependencies
- Working with large packages that take time to download
- Operating in environments with limited bandwidth
- Running CI/CD pipelines where package caching can significantly improve build times

### Unpacking without `pixi-pack`

If you don't have `pixi-pack` available on your target system, you can still install the environment if you have `conda` or `micromamba` available.
Just unarchive the `environment.tar`, then you have a local channel named `pixi-pack` on your system where all necessary packages are available.
Next to this local channel, you will find an `environment.yml` file that contains the environment specification.
You can then install the environment using `conda` or `micromamba`:

```bash
tar -xvf environment.tar
micromamba create -p ./env --file environment.yml
# or
conda env create -p ./env --file environment.yml
```

> [!NOTE]
> The `environment.yml` and `repodata.json` files are only for this use case, `pixi-pack unpack` does not use them.

> [!NOTE]
> Both `conda` and `mamba` are always installing pip as a side effect when they install python, see [`conda`'s documentation](https://docs.conda.io/projects/conda/en/25.1.x/user-guide/configuration/settings.html#add-pip-as-python-dependency-add-pip-as-python-dependency).
> This is different from how `pixi` works and can lead to solver errors when using `pixi-pack`'s compatibility mode since `pixi-pack` doesn't include `pip` by default.
> You can fix this issue in two ways:
>
> - Add `pip` to your `pixi.lock` file using `pixi add pip`.
> - Configuring `conda` (or `mamba`) to not install `pip` by default by running `conda config --set add_pip_as_python_dependency false` (or by adding `add_pip_as_python_dependency: False` to your `~/.condarc`)
