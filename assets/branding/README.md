# AppDock logo

`appdock.png` is the selected blue Dock logo, copied unchanged from
`output/logo-variants/01-dock.png`. The generation and refinement prompts are in
`output/logo-variants/01-dock-prompt.txt`.

`scripts/package.sh` creates the standard macOS icon sizes from this source and
embeds `AppDock.icns` in the application bundle. Both debug and release bundles
use this icon.

The executable also embeds the PNG and sets its application icon at startup,
so direct launches such as `cargo run` use the same logo without a bundle.
