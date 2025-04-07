# PyPI Example

`pixi-pack` does not support PyPI source distributions.
To still pack the environment (excluding PyPI source distributions), you can run:

```shell
pixi-pack pack --ignore-pypi-non-wheel
```
