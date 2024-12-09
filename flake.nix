{
  description = "iroh-doctor";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-24.11";
    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
          config.android_sdk.accept_license = true;
          config.allowUnfree = true;
        };

        androidenvPkgs = import nixpkgs {
          inherit system overlays;
          config.android_sdk.accept_license = true;
          config.allowUnfree = true;
        };

        nightly-rustfmt = pkgs.rust-bin.nightly.latest.rustfmt;

        androidComposition = androidenvPkgs.androidenv.composeAndroidPackages {
          platformVersions = [ "34" ];
          buildToolsVersions = [ "34.0.0" ];
          includeEmulator = false; # haven't figured it out yet...
          includeNDK = true;
          # may need to wait for https://github.com/NixOS/nixpkgs/pull/300386 to land
          ndkVersion = "26.1.10909125";
        };
      in {
        devShells.default = pkgs.mkShell rec {
          name = "iroh-doctor";
          nativeBuildInputs = with pkgs;
            [
              nightly-rustfmt
              direnv
              corepack # includes pnpm
              pkg-config
              # c libraries needed for tauri on linux desktop
              openssl
              glib.dev
              pango.dev
              libsoup_3.dev
              webkitgtk_4_1.dev
              # needed for rust android compilation (pnpm tauri android dev)
              llvmPackages_13.libcxx
              libxml2
              jdk17
              code-cursor
              # android development tools
              androidComposition.androidsdk
            ] ++ lib.optionals stdenv.isDarwin [
              darwin.apple_sdk.frameworks.Security
              darwin.apple_sdk.frameworks.CoreFoundation
              darwin.apple_sdk.frameworks.Foundation
            ];

          # env variables so tauri picks up our android sdk install
          ANDROID_SDK_ROOT =
            "${androidComposition.androidsdk}/libexec/android-sdk";
          ANDROID_NDK_ROOT = "${ANDROID_SDK_ROOT}/ndk-bundle";
          ANDROID_HOME = "${ANDROID_SDK_ROOT}";
          NDK_HOME = "${ANDROID_NDK_ROOT}";

          # For some reason that's needed for the android NDK's clang setup to work
          LD_LIBRARY_PATH = "${pkgs.libxml2.out}/lib";

          # Needed for `tauri android dev` to pick up the jdk
          JAVA_HOME = "${pkgs.jdk17}/lib/openjdk";

          # Fixes an empty window bug for me https://github.com/tauri-apps/tauri/issues/8254
          WEBKIT_DISABLE_COMPOSITING_MODE = 1;
        };
      });
}
