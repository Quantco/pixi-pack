Output .github/assets/demo/demo.mp4
Output .github/assets/demo/demo.gif
Require pixi-pack

Set Shell "bash"
Set FontSize 32
Set Width 1920
Set Height 1080

Hide
# Type "pushd examples/simple-python" Enter
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
Sleep 3s

Type@200ms "ls" Sleep 1s Enter
Sleep 4s

Type@200ms "source activate.sh" Sleep 1s Enter
Sleep 3s

Type@200ms "which python" Sleep 1s Enter
Sleep 5s
