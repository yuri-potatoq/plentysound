{
  description = "Rust development environment with crane";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    flake-parts.url = "github:hercules-ci/flake-parts";

    crane.url = "github:ipetkov/crane";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  nixConfig = {
    extra-substituters = [
      "https://cache.nixos.org"
      "https://nix-community.cachix.org"
      "https://crane.cachix.org"
    ];
    extra-trusted-public-keys = [
      "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCUSeBo="
      "crane.cachix.org-1:8Scfpmn9w+hGdXH/Q9tTLiYAE/2dnJYRJP7kl80GuRk="
    ];
  };

  outputs = inputs@{ flake-parts, nixpkgs, crane, fenix, advisory-db, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];

      perSystem = { system, pkgs, ... }:
        let
          toolchain = (fenix.packages.${system}.toolchainOf {
            channel = "1.93.1";
            sha256 = "sha256-SBKjxhC6zHTu0SyJwxLlQHItzMzYZ71VCWQC2hOzpRY=";
          }).toolchain;

          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          src = craneLib.cleanCargoSource ./.;

          libvosk = pkgs.callPackage ./nix/libvosk.nix { };

          commonPkgArgs = { inherit craneLib src; };

          plentysound = pkgs.callPackage ./nix/package.nix commonPkgArgs;

          plentysound-full = pkgs.callPackage ./nix/package.nix (commonPkgArgs // {
            inherit libvosk;
            enableTranscriber = true;
          });

          devBuildInputs = with pkgs; [
            # Rust tooling
            rust-analyzer

            # Build tooling
            pkg-config
            openssl
            libvosk
            pipewire
            dbus
            llvmPackages.libclang

            # Dev utilities
            cargo-watch
            just
          ] ++ lib.optionals stdenv.isDarwin [
            libiconv
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];
        in
        {
          packages = {
            inherit plentysound plentysound-full;
            default = plentysound;
          };

          checks = {
            inherit plentysound;

            clippy = craneLib.cargoClippy (plentysound.passthru.baseArgs // {
              inherit (plentysound.passthru) cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            });

            tests = craneLib.cargoTest (plentysound.passthru.baseArgs // {
              inherit (plentysound.passthru) cargoArtifacts;
            });

            fmt = craneLib.cargoFmt { inherit src; };

            audit = craneLib.cargoAudit {
              inherit src advisory-db;
            };
          };

          devShells.default = craneLib.devShell {
            checks = {
              inherit plentysound;
            };
            shellHook = ''
              export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath devBuildInputs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
              export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
            '';

            packages = devBuildInputs;

            RUST_BACKTRACE = "1";
            RUST_LOG = "debug";
          };
        };
    };
}
