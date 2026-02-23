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

          libvosk = pkgs.stdenv.mkDerivation {
            pname = "libvosk";
            version = "0.3.45";
            src = pkgs.fetchurl {
              url = "https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-linux-x86_64-0.3.45.zip";
              sha256 = "sha256-u9yO2FxDl59kQxQoiXcOqVy/vFbP+1xdzXOvqHXF+7I=";
            };
            nativeBuildInputs = [ pkgs.unzip pkgs.autoPatchelfHook ];
            buildInputs = [ pkgs.stdenv.cc.cc.lib ];
            unpackPhase = "unzip $src";
            installPhase = ''
              mkdir -p $out/lib $out/include
              cp vosk-linux-x86_64-0.3.45/libvosk.so $out/lib/
              cp vosk-linux-x86_64-0.3.45/vosk_api.h $out/include/
            '';
          };

          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          src = craneLib.cleanCargoSource ./.;

          commonArgs = {
            inherit src;
            strictDeps = true;

            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [
              libvosk
              pkgs.pipewire
              pkgs.dbus
            ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
              pkgs.darwin.apple_sdk.frameworks.Security
              pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
            ];
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          myCrate = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
          });

          buildInputs = with pkgs; [
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
          packages.default = myCrate;

          checks = {
            inherit myCrate;

            clippy = craneLib.cargoClippy (commonArgs // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            });

            tests = craneLib.cargoTest (commonArgs // {
              inherit cargoArtifacts;
            });

            fmt = craneLib.cargoFmt { inherit src; };

            audit = craneLib.cargoAudit {
              inherit src advisory-db;
            };
          };

          devShells.default = craneLib.devShell {
            checks = {
              inherit myCrate;
            };
            shellHook = ''
              export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath buildInputs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
              export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
            '';

            packages = buildInputs;

            RUST_BACKTRACE = "1";
            RUST_LOG = "debug";
          };
        };
    };
}
