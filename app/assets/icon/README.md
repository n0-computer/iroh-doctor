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
