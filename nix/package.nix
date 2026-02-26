{
  craneLib,
  src,
  lib,
  stdenv,
  pkg-config,
  autoPatchelfHook,
  pipewire,
  dbus,
  llvmPackages,
  glibc,
  libiconv ? null,
  darwin ? null,
  libvosk ? null,
  enableTranscriber ? false,
}:

let
  # Vendor filtering is now handled automatically by the crane overlay
  # See nix/crane-overlay.nix for the implementation

  baseArgs = {
    inherit src;
    pname = "plentysound";
    version = "0.1.0";
    strictDeps = true;

    nativeBuildInputs = [
      pkg-config
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

    LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
    BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${glibc.dev}/include";

    # Skip building documentation for release builds (faster)
    CARGO_BUILD_RUSTDOC = "false";

    # Use --offline to build without network access (prevents re-adding Windows deps)
    cargoExtraArgs = lib.concatStringsSep " " (
      [ "--offline" ] ++
      (if enableTranscriber then [ "--features" "transcriber" ] else [ "-p" "plentysound" ])
    );
  } // lib.optionalAttrs enableTranscriber {
    LD_LIBRARY_PATH = lib.makeLibraryPath [ libvosk ];
  };

  # Build dependencies
  cargoArtifacts = craneLib.buildDepsOnly baseArgs;
in
craneLib.buildPackage (baseArgs // {
  inherit cargoArtifacts;

  passthru = { inherit baseArgs cargoArtifacts; };
})
