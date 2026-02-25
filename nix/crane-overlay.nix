# Crane overlay to create minimal stub derivations for Windows packages
# This avoids downloading ~180MB of Windows-only code that we never use
#
# Usage in flake.nix:
#   craneLib = (crane.mkLib pkgs).overrideScope (import ./nix/crane-overlay.nix { inherit pkgs; })

{ pkgs }:

craneScope: cranePrev:

let
  inherit (pkgs) lib;

  # Patterns to identify Windows-specific crates
  windowsPatterns = [
    "^winapi(-.*)?$"
    "^windows-sys(-.*)?$"
    "^windows-.*$"
    "^windows_.*$"
    "^crossterm_winapi(-.*)?$"
  ];

  # Check if a package name matches Windows patterns
  isWindowsPackage = name:
    lib.any (pattern: builtins.match pattern name != null) windowsPatterns;

  # Generate crates.io-index path for a crate name
  # Path structure: 1/<name>, 2/<name>, 3/<c>/<name>, or <cc>/<cc>/<name>
  crateIndexPath = name:
    let
      len = builtins.stringLength name;
    in
      if len == 1 then "1/${name}"
      else if len == 2 then "2/${name}"
      else if len == 3 then "3/${builtins.substring 0 1 name}/${name}"
      else "${builtins.substring 0 2 name}/${builtins.substring 2 2 name}/${name}";

  # Metadata hashes for Windows crates from crates.io-index
  # These change rarely, only when new versions are published
  metadataHashes = {
    "crossterm_winapi" = lib.fakeHash;
    "winapi" = lib.fakeHash;
    "winapi-i686-pc-windows-gnu" = lib.fakeHash;
    "winapi-x86_64-pc-windows-gnu" = lib.fakeHash;
    "windows-sys" = lib.fakeHash;
    "windows-targets" = lib.fakeHash;
    "windows-link" = lib.fakeHash;
  };

  # Create minimal stub derivation using crates.io-index metadata
  # Fetches only small JSON file (~10KB) instead of full .crate tarball (~1MB+)
  createMinimalStub = { name, version, checksum, ... }:
    let
      indexPath = crateIndexPath name;
      metadataUrl = "https://raw.githubusercontent.com/rust-lang/crates.io-index/master/${indexPath}";

      # Fetch metadata JSON using same pattern as crane's downloadCargoPackage
      metadataFile = pkgs.fetchurl {
        url = metadataUrl;
        name = "${name}-metadata.json";
        sha256 = metadataHashes.${name} or lib.fakeHash;
      };

      # Parse NDJSON (one JSON object per line)
      rawContent = builtins.readFile metadataFile;
      lines = lib.filter (l: l != "") (lib.splitString "\n" rawContent);
      allVersions = map builtins.fromJSON lines;
      ourVersion = lib.findFirst (v: v.vers == version) null allVersions;
      features = if ourVersion != null then ourVersion.features else {};

      # Convert features to TOML format
      featuresToml = lib.concatStringsSep "\n" (
        lib.mapAttrsToList (fname: fdeps:
          "${fname} = [${lib.concatMapStringsSep ", " (d: ''"${d}"'') fdeps}]"
        ) features
      );
    in
    pkgs.runCommandLocal "cargo-package-${name}-${version}" {} ''
      mkdir -p $out/src

      cat > $out/Cargo.toml <<EOF
      [package]
      name = "${name}"
      version = "${version}"
      edition = "2021"

      [lib]
      path = "src/lib.rs"

      [features]
      ${featuresToml}
      EOF

      # Create stub source files
      echo "// Minimal stub for Windows-only crate ${name}" > $out/src/lib.rs

      # Stub build.rs
      echo "fn main() {}" > $out/build.rs

      # Create checksum file
      echo '{"files":{},"package":"${checksum}"}' > $out/.cargo-checksum.json
    '';
in
{
  # Override downloadCargoPackage to create stubs for Windows packages
  # This intercepts at the lowest level before any downloads happen
  downloadCargoPackage = args:
    if isWindowsPackage args.name then
      createMinimalStub args
    else
      cranePrev.downloadCargoPackage args;
}
