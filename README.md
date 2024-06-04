# `pixi-pack`

<br />
<p align="center">
  <h3 align="center">📦 A tool to pack and unpack conda environments created with pixi</h3>
</p>

## 🗂 Table of Contents

- [Introduction](#-introduction)
- [Installation](#-installation)
- [Usage](#-usage)

## 📖 Introduction

Starting with a [pixi](https://pixi.sh) lockfile `pixi.lock`, you can create a packed environment that can be shared with others.
This environment can be unpacked on any system using `pixi-pack` to recreate the original environment.

In contrast to [`conda-pack`](https://conda.github.io/conda-pack/), `pixi-pack` does not require the original conda environment to be present on the system for packing.
Instead, it uses the lockfile to download the required packages and puts them into a `.tar.zstd` archive.
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

## 🎯 Usage

### `pixi-pack pack`: Packing an environment

With `pixi-pack pack`, you can pack a conda environment into a `environment.tar.zstd` file:

```bash
pixi-pack pack --manifest-file pixi.toml --environment prod --platform linux-64
```

This will create a `environment.tar.zstd` file that contains all conda packages required to create the environment.

```
# environment.tar.zstd
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

With `pixi-pack unpack environment.tar.zstd`, you can unpack the environment on your target system.
This will create a new conda environment in `./env` that contains all packages specified in your `pixi.toml`.
It also creates an `activate.sh` (or `activate.bat` on Windows) file that lets you activate the environment
without needing to have `conda` or `micromamba` installed.

```bash
$ pixi-pack unpack environment.tar.zstd
$ ls
env/
activate.sh
environment.tar.zstd
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

### Inject additional packages

You can inject additional packages into the environment that are not specified in `pixi.lock` by using the `--inject` flag:

```bash
pixi-pack pack --inject local-package-1.0.0-hbefa133_0.conda --manifest-pack pixi.toml
```

This can be particularly useful if you build the project itself and want to include the built package in the environment but still want to use `pixi.lock` from the project.

### Unpacking without `pixi-pack`

If you don't have `pixi-pack` available on your target system, you can still install the environment if you have `conda` or `micromamba` available.
Just decompress the `environment.tar.zstd`, then you have a local channel on your system where all necessary packages are available.
Next to this local channel, you will find an `environment.yml` file that contains the environment specification.
You can then install the environment using `conda` or `micromamba`:

```bash
tar --zstd -xvf environment.tar.zstd
micromamba create -p ./env --file environment.yml"
# or
conda env create -p ./env --file environment.yml"
```

> [!NOTE]
> The `environment.yml` and `repodata.json` files are only for this use case, `pixi-pack unpack` does not use them.
