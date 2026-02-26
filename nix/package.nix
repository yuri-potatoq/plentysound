{
  craneLib,
  src,
  lib,
  stdenv,
  pkg-config,
  autoPatchelfHook,
  pipewire,
  dbus,
  llvmPackages_19,  # Use LLVM 19 instead of 21 for smaller builds
  glibc,
  libiconv ? null,
  darwin ? null,
  libvosk ? null,
  enableTranscriber ? false,
}:

let
  # Vendor filtering is now handled automatically by the crane overlay
  # See nix/crane-overlay.nix for the implementation

  # Common settings shared by dependency and binary builds
  commonArgs = {
    inherit src;
    pname = "plentysound";
    version = "0.1.0";
    strictDeps = true;

    nativeBuildInputs = [
      pkg-config
      llvmPackages_19.libclang  # Build-time only, not in runtime closure
    ] ++ lib.optionals stdenv.isLinux [
      autoPatchelfHook
    ];

    buildInputs = [
      pipewire
      dbus
      stdenv.cc.cc.lib  # Provides libgcc_s for autoPatchelfHook
    ] ++ lib.optionals stdenv.isDarwin [
      libiconv
      darwin.apple_sdk.frameworks.Security
      darwin.apple_sdk.frameworks.SystemConfiguration
    ] ++ lib.optionals enableTranscriber [
      libvosk
    ];

    LIBCLANG_PATH = "${llvmPackages_19.libclang.lib}/lib";
    BINDGEN_EXTRA_CLANG_ARGS = builtins.concatStringsSep " " [
      "-isystem ${glibc.dev}/include"
      "-isystem ${llvmPackages_19.libclang.lib}/lib/clang/19/include"
    ];

    cargoExtraArgs = lib.concatStringsSep " " (
      [ "--offline" ] ++
      (if enableTranscriber then [ "--features" "transcriber" ] else [ "-p" "plentysound" ])
    );
  } // lib.optionalAttrs enableTranscriber {
    LD_LIBRARY_PATH = lib.makeLibraryPath [ libvosk ];
  };

  # Aggressive size optimizations for dependencies
  # These are built once and cached, so we prioritize size over compile speed
  depsArgs = commonArgs // {
    CARGO_PROFILE = "release";
    CARGO_BUILD_RUSTDOC = "false";
    CARGO_PROFILE_RELEASE_OPT_LEVEL = "z";
    CARGO_PROFILE_RELEASE_LTO = "thin";
    CARGO_PROFILE_RELEASE_STRIP = "symbols";
    doCheck = false;  # Don't run tests during dependency build
    cargoTestExtraArgs = "--all-targets";  # Skip doc tests if doCheck is enabled
  };

  # Settings for final binary
  # Inherits release profile from deps but can have different opt-level
  binaryArgs = commonArgs // {
    CARGO_PROFILE = "release";
    CARGO_BUILD_RUSTDOC = "false";
    CARGO_PROFILE_RELEASE_OPT_LEVEL = "z";
    CARGO_PROFILE_RELEASE_LTO = "thin";
    CARGO_PROFILE_RELEASE_STRIP = "symbols";
    dontStrip = false;
    doCheck = false;
    cargoTestExtraArgs = "--all-targets";
  };

  cargoArtifacts = craneLib.buildDepsOnly depsArgs;
in
craneLib.buildPackage (binaryArgs // {
  inherit cargoArtifacts;

  passthru = {
    inherit commonArgs depsArgs binaryArgs cargoArtifacts;
    # Keep baseArgs for backwards compatibility with checks
    baseArgs = commonArgs;
  };
})
