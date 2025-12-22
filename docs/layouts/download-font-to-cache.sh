#!/bin/bash

# Since Moranga is not on Google Fonts, we have to download the TTF file ourselves
# and place it in the cache for the social card plugin to use it.
# This step is executed by Pixi

mkdir -p .cache/plugin/social/fonts/Moranga
curl -sL https://fonts.prefix.dev/moranga/MorangaMedium/font.ttf \
    -o .cache/plugin/social/fonts/Moranga/Medium.ttf
