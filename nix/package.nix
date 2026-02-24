{
  craneLib,
  src,
  lib,
  pkg-config,
  pipewire,
  dbus,
  llvmPackages,
  glibc,
  libiconv ? null,
  darwin ? null,
  stdenv,
  libvosk ? null,
  enableTranscriber ? false,
}:

let
  baseArgs = {
    inherit src;
    pname = "plentysound";
    version = "0.1.0";
    strictDeps = true;

    nativeBuildInputs = [ pkg-config ];
    buildInputs = [
      pipewire
      dbus
    ] ++ lib.optionals stdenv.isDarwin [
      libiconv
      darwin.apple_sdk.frameworks.Security
      darwin.apple_sdk.frameworks.SystemConfiguration
    ] ++ lib.optionals enableTranscriber [
      libvosk
    ];
    LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
    BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${glibc.dev}/include";
    cargoExtraArgs = if enableTranscriber
      then "--features transcriber"
      else "-p plentysound";
  } // lib.optionalAttrs enableTranscriber {
    LD_LIBRARY_PATH = lib.makeLibraryPath [ libvosk ];
  };

  cargoArtifacts = craneLib.buildDepsOnly baseArgs;
in
craneLib.buildPackage (baseArgs // {
  inherit cargoArtifacts;

  passthru = { inherit baseArgs cargoArtifacts; };
})
