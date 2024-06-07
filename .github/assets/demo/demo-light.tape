Output .github/assets/demo/demo-light.mp4
Output .github/assets/demo/demo-light.gif

Set Theme "rose-pine-dawn"
Set Shell "bash"
Set FontSize 32
Set Width 1920
Set Height 700

Hide
Type "export PATH=$(pwd)/target/debug:$PATH" Enter
Type "pushd examples/simple-python" Enter
Type "rm -rf env/ environment.tar activate.*" Enter
Type "clear" Enter
Show

Type@200ms "ls" Sleep 1s Enter
Sleep 3s

Type@200ms "pixi-pack pack" Sleep 1s Enter
Sleep 5s

Type "ls" Sleep 1s Enter
Sleep 3s

Type@200ms "pixi-pack unpack environment.tar" Sleep 1s Enter
Sleep 4s

Type@200ms "ls" Sleep 1s Enter
Sleep 4s

Type@200ms "source activate.sh" Sleep 1s Enter
Sleep 3s

Type@200ms "which python" Sleep 1s Enter
Sleep 10s
