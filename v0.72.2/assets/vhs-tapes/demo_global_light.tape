#!/usr/bin/env pixi exec -s vhs -s ffmpeg -s ttyd vsh

Output pixi_global_demo_light.gif

Require pixi

Set Shell "zsh"
Set FontSize 32
Set Width 1920
Set Height 600
Set Theme "rose-pine-dawn"
Set TypingSpeed 40ms
Set CursorBlink false

# Reset the installed packages
Hide
Type "pixi global uninstall gh nvim ipython btop ripgrep" Enter
Sleep 3s
Type "clear" Enter
Sleep 1s
Show

Type "pixi global install gh nvim ipython btop ripgrep" Sleep 500ms  Enter

Sleep 5s
