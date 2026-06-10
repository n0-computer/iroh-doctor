# App icon assets

Source of truth and generated raster sets for the **iroh doctor** app icon.
Motif: a white ECG/pulse line on the iroh brand-purple (`#7C7CFF`) gradient,
echoing the app's live RTT sparkline — a "doctor" reading connection health.

## Files

- `icon-master.svg` / `icon-master.png` — full-bleed 1024² master. Source for the
  iOS App Store icon and all square raster sizes.
- `icon-foreground.svg` / `icon-foreground.png` — Android adaptive-icon foreground
  (transparent; motif scaled to ~62% so it survives the safe-zone crop).
- `ios/AppIcon.appiconset/` — iOS icon set + `Contents.json` (drop into an Xcode
  asset catalog / `actool`).
- `android/mipmap-*/` — `ic_launcher.png` (legacy square) and
  `ic_launcher_foreground.png` (adaptive) per density.
- `android/mipmap-anydpi-v26/ic_launcher.xml` — adaptive-icon definition.
- `android/values/colors.xml` — `ic_launcher_background` brand-purple color.

## Regenerating

`*.png` are rendered from the `*.svg` with macOS `qlmanage` (WebKit, accurate)
and resized with `sips`. ImageMagick's SVG renderer mangles the gradient/polyline
here — do not use it for these. To re-render the master:

```sh
qlmanage -t -s 1024 -o . icon-master.svg && mv icon-master.svg.png icon-master.png
```

## Wiring status (IMPORTANT — dx 0.7.9)

`dx` does **not** generate the mobile app icon from `[bundle] icon` in 0.7.9
(verified: setting it left the iOS `.app` with no `AppIcon` and the Android
launcher as the default green robot). `[bundle] icon` only affects desktop
bundles. So these assets are **not yet wired into the device builds**.

To ship the real icon, inject these into the dx-generated native projects (under
`target/dx/iroh-doctor-app/<profile>/<platform>/`) before the native build:

- **iOS**: add `ios/AppIcon.appiconset` to the app's asset catalog and reference
  it (`actool` / Xcode), or add `CFBundleIcons` → `CFBundlePrimaryIcon` to the
  Info.plist via `[ios.raw.info_plist]` pointing at icon files copied into the
  bundle.
- **Android**: copy `android/mipmap-*` + `mipmap-anydpi-v26/ic_launcher.xml` +
  `values/colors.xml` over the generated `res/` (which dx regenerates, so this
  needs a build hook, not a one-time edit).

Because the generated projects live under `target/` and are regenerated, the
durable fix is a small pre-build step (or a newer dx with mobile-icon support).
Tracked in `plans/app-store-release-design.md` (Phase 2/3).

## Splash / launch screen

dx 0.7.9 has no splash config. Its iOS Info.plist template hardcodes
`UILaunchStoryboardName` = `LaunchScreen` but generates no storyboard, so the
launch screen is blank unless we compile one in. `scripts/bundle-mobile.sh`
handles both platforms:

- **iOS**: `../splash/ios/LaunchScreen.storyboard` (brand-purple background,
  centered white pulse) is compiled with `xcrun ibtool` into
  `LaunchScreen.storyboardc` inside the `.app`. The mark it references is the
  `LaunchIcon.imageset` in `ios/Assets.xcassets`, compiled by the existing
  actool step. Needs a machine with Xcode; unverified on a device so far.
- **Android 12+**: the script copies `../splash/android/values-v31/styles.xml`
  into the generated `res/`, overriding `AppTheme` with
  `windowSplashScreenBackground` in brand purple. The splash icon is the
  launcher adaptive icon the script already injects. Pre-31 Android has no
  system splash; we accept the default window background there.

The `LaunchIcon` PNGs render from `../splash/splash-mark.svg` (the pulse with
a tight viewBox) at 240/480/720 px wide:

```sh
cd app/assets
for w in 240 480 720; do
  suffix=""; [ $w = 480 ] && suffix=@2x; [ $w = 720 ] && suffix=@3x
  rsvg-convert -w $w -o "icon/ios/Assets.xcassets/LaunchIcon.imageset/LaunchIcon$suffix.png" splash/splash-mark.svg
done
```

## Store assets and favicon

`../store/feature-graphic.svg` is the 1024x500 Google Play feature graphic
(gradient + pulse from `icon-master.svg`, wordmark in the system Avenir Next
stack). `../favicon.ico` holds 16/32/48 px renderings of the master icon and
is what `app/src/main.rs` serves as the in-app favicon. Both render with
`rsvg-convert` (librsvg, installed via homebrew), which reproduces the
gradient and rounded strokes faithfully here; the ImageMagick warning above
applies only to ImageMagick's own SVG decoder. qlmanage still works for the
square icons but crops non-square canvases, so use `rsvg-convert` for the
feature graphic:

```sh
cd app/assets
rsvg-convert -w 1024 -h 500 -o store/feature-graphic.png store/feature-graphic.svg
for s in 16 32 48; do rsvg-convert -w $s -h $s -o /tmp/fav-$s.png icon/icon-master.svg; done
magick /tmp/fav-16.png /tmp/fav-32.png /tmp/fav-48.png favicon.ico
```
