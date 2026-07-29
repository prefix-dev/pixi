# Since Moranga is not on Google Fonts, we have to download the TTF file ourselves
# and place it in the cache for the social card plugin to use it.
# This step is executed by Pixi

let path = ".cache/plugin/social/fonts/Moranga/Medium.ttf"

if not ($path | path exists) {
    mkdir ($path | path dirname)
    http get https://fonts.prefix.dev/moranga/MorangaMedium/font.ttf
        | save $path
}
