Output .github/assets/demo/demo-dark.mp4
Output .github/assets/demo/demo-dark.gif

Set Theme "rose-pine-moon"
Set Shell "bash"
Set FontSize 32
Set Width 1920
Set Height 750

Hide
Type "export PATH=$(pwd)/target/debug:$PATH" Enter
Type "rm -rf $HOME/pixi-pack-example" Enter
Type "mkdir -p $HOME/pixi-pack-example" Enter
Type "cp examples/simple-python/pixi.* $HOME/pixi-pack-example" Enter
Type "pushd $HOME/pixi-pack-example" Enter
Type "clear" Enter
Sleep 1s
Show

Type@150ms "ls" Sleep 1s Enter
Sleep 3s

Type@150ms "pixi-pack pack" Sleep 1s Enter
Sleep 5s

Type "ls" Sleep 1s Enter
Sleep 3s

Type@150ms "pixi-pack unpack environment.tar" Sleep 1s Enter
Sleep 4s

Type@150ms "ls" Sleep 1s Enter
Sleep 4s

Type@150ms "source activate.sh" Sleep 1s Enter
Sleep 3s

Type@150ms "which python" Sleep 1s Enter
Sleep 10s
