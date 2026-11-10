# Unofficial macOS fork

This is a personal, unofficial fork of
[anefzaoui/misterdownloadmanager](https://github.com/anefzaoui/misterdownloadmanager), made to
run the app with a native macOS shell. It is not endorsed by the upstream author, and the
upstream project is not responsible for it. The GPL-3.0-or-later licence of the original
applies to the whole repository (see [LICENSE](LICENSE) and [ATTRIBUTION.md](ATTRIBUTION.md)).

Base: upstream commit `87074c69` ("A link from the browser: ask again as MDM when its
User-Agent is refused"), 2026-11-09; changes dated 2026-11-10.

## What this fork changes

- **Native macOS main window** (`crates/app/src/app/macos_main.rs`, new): sidebar with
  categories, an icon toolbar, a grouped download list with columns and a details inspector,
  a status bar, and a speed card. The layout follows AppKit conventions (system title bar and
  traffic lights, 24–30 pt controls, hairlines instead of etched frames).
- **Menus drawn inside the window** (`chrome.rs`): a rounded card with a shadow, accent
  highlighting, submenu flipping, instead of borderless menu windows (which cannot be
  transparent with eframe's process-wide backbuffer setting).
- **macOS control skin** (`palette.rs`, `native.rs`, `winui.rs`): flat buttons without the
  Windows 11 bottom rule, a macOS checkbox/radio, rounded fields, hairline separators, capsule
  progress bars and segmented tabs; Windows/Linux metrics are unchanged.
- **Dialogs rebuilt for macOS**: Options as a sidebar window, Add URL, Properties, the
  progress window with a segmented control, the torrent windows, delete confirmations.
- **Localisation fixes**: macOS-only texts go through `crate::i18n`, the extractor scans
  `src/**/*.rs`, and the Simplified Chinese catalogue covers the new keys.
- **Version display**: the window title and About show the build date
  (`Mister Download Manager 2026-11-10`) from `MDM_BUILD_DATE`; the Cargo version stays the
  one update checks compare with the releases.
- **Scripted UI checks** (`crates/app/src/uitest.rs`, debug builds only): `MDM_UI_SCRIPT`
  injects clicks, keys and text into egui's raw input, so screenshots and checks never move
  the real pointer or keyboard.
- **Packaging**: `tools/installer/build-macos.sh` takes `MDM_MACOS_TARGETS=arm64` (or `x86_64`,
  `all`), so a local build can produce only the Apple Silicon disk image.

## Building

```bash
cargo build -p mdm-app
cargo test -p mdm-app
MDM_MACOS_TARGETS=arm64 tools/installer/build-macos.sh   # target/dist/*.dmg
```

The disk images are ad-hoc signed: macOS asks before the first start (right-click the app →
Open). A Developer ID (`MDM_MACOS_IDENTITY`) and notarisation profile (`MDM_NOTARY_PROFILE`)
make them ordinary signed apps.
