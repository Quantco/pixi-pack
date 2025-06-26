Output .github/assets/demo/demo-light.mp4
Output .github/assets/demo/demo-light.gif

Set Theme "rose-pine-dawn"
Set Shell "bash"
Set FontSize 32
Set Width 1920
Set Height 750

Hide
Type "export PATH=$(pwd)/target/release:$PATH" Enter
Type "rm -rf $HOME/pixi-pack-example" Enter
Type "mkdir -p $HOME/pixi-pack-example" Enter
Type "cp examples/simple-python/pixi.* $HOME/pixi-pack-example" Enter
Type "pushd $HOME/pixi-pack-example" Enter
Type "clear" Enter
Sleep 1s
Show

Type@150ms "ls" Sleep 1s Enter
Sleep 3s

Type@150ms "pixi-pack" Sleep 1s Enter
Wait+Screen@10s /Created pack/

Type "ls" Sleep 1s Enter
Sleep 3s

Type@150ms "pixi-unpack environment.tar" Sleep 1s Enter
Wait+Screen@10s /Finished unpacking/

Type@150ms "ls" Sleep 1s Enter
Sleep 4s

Type@150ms "source activate.sh" Sleep 1s Enter
Sleep 3s

Type@150ms "which python" Sleep 1s Enter
Sleep 10s
