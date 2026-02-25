{ lib, stdenv, findutils }:

{
  # Filters a crane vendor directory to remove platform-specific crates
  # Usage: filterVendorDir { vendorDir = ...; filterPatterns = [...]; }
  filterVendorDir = {
    vendorDir,
    name ? "vendor-filtered",
    filterPatterns ? [
      "winapi-*"
      "windows-sys-*"
      "windows-*"
      "windows_*"
      "crossterm_winapi-*"
    ]
  }:
    stdenv.mkDerivation {
      inherit name;
      src = vendorDir;

      nativeBuildInputs = [ findutils ];

      buildPhase =
        let
          findArgs = lib.concatMapStringsSep " -o " (p: "-name \"${p}\"") filterPatterns;
        in ''
          # Copy vendor directory structure (resolve all symlinks)
          cp -rL $src vendor
          chmod -R u+w vendor
          cd vendor

          # Find and filter Windows-specific crates recursively
          echo "Filtering vendor directory with patterns: ${toString filterPatterns}"

          # Count total directories before filtering
          BEFORE=$(find . -type d -name "*-*" | wc -l)

          # Remove filtered crates by name pattern (search at depth 1 and 2 to handle different structures)
          find . -mindepth 1 -maxdepth 2 -type d \( ${findArgs} \) -exec rm -rf {} + 2>/dev/null || true

          # Count after filtering
          AFTER=$(find . -type d -name "*-*" | wc -l)
          REMOVED=$((BEFORE - AFTER))

          echo "Removed $REMOVED Windows-specific crate directories"
          echo "Remaining crate directories: $AFTER"
        '';

      installPhase = ''
        mkdir -p $out
        cp -r . $out/
      '';
    };
}
