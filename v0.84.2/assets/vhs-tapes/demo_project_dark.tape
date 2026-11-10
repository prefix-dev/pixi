#!/usr/bin/env pixi exec -s vhs -s ffmpeg -s ttyd vsh

Output pixi_project_demo_dark.gif

Require pixi

Set Shell "zsh"
Set FontSize 32
Set Width 1920
Set Height 500
Set Theme "rose-pine-moon"
Set TypingSpeed 40ms
Set CursorBlink false

# Reset the installed packages
Hide
Type "unset PIXI_PROJECT_MANIFEST" Enter
Type "rm -rf /tmp/hello-world" Enter
Type "cd /tmp" Enter
Sleep 2s
Type "clear" Enter
Sleep 1s
Show

Type "pixi init hello-world" Sleep 500ms  Enter Sleep 500ms
Type "cd hello-world" Sleep 500ms  Enter Sleep 500ms
Type "pixi add python" Sleep 500ms  Enter
Sleep 2s
Type `pixi run python -c 'print("Hello World!")'` Sleep 500ms  Enter
Sleep 3s
