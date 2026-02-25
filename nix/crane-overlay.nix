# Crane overlay to filter Windows dependencies from Cargo.lock before vendoring
# This prevents downloading Windows cargo packages entirely
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

  # Filter Cargo.lock to remove Windows dependencies using Python script
  filterCargoLock = { src, cargoLock ? null }:
    let
      lockFile = if cargoLock != null then cargoLock else "${src}/Cargo.lock";

      # Build regex pattern for Windows packages
      # Use .* to match any suffix (e.g., winapi-i686-pc-windows-gnu)
      windowsPatternsRegex = lib.concatStringsSep "|" [
        "winapi.*"
        "windows-sys.*"
        "windows-link.*"
        "windows-targets.*"
        "windows_.*"
        "crossterm_winapi.*"
      ];

      filterScript = pkgs.writeScript "filter-cargo-lock.py" ''
        #!${pkgs.python3}/bin/python3
        import re
        import sys

        # Read original Cargo.lock
        with open(sys.argv[1], "r") as f:
            content = f.read()

        # Patterns for Windows packages
        windows_patterns = re.compile(r'^name = "(${windowsPatternsRegex})"', re.MULTILINE)

        # Split into package blocks
        packages = content.split('\n[[package]]')
        header = packages[0]
        package_blocks = packages[1:]

        filtered_blocks = []
        windows_package_names = set()

        # First pass: identify Windows packages
        for block in package_blocks:
            if windows_patterns.search(block):
                # Extract package name
                name_match = re.search(r'^name = "([^"]+)"', block, re.MULTILINE)
                if name_match:
                    windows_package_names.add(name_match.group(1))
            else:
                filtered_blocks.append(block)

        # Second pass: remove Windows dependencies from remaining packages
        final_blocks = []
        for block in filtered_blocks:
            # Find dependencies section
            deps_match = re.search(r'(dependencies = \[)(.*?)(\])', block, re.DOTALL)
            if deps_match:
                deps_content = deps_match.group(2)
                # Filter out Windows dependencies
                deps_lines = deps_content.strip().split('\n')
                filtered_deps = []
                for line in deps_lines:
                    line = line.strip()
                    if not line or line == ',':
                        continue
                    # Remove trailing comma if present
                    line = line.rstrip(',').strip()
                    if not line:
                        continue
                    # Extract dependency name (handle both "name" and "name version" formats)
                    dep_match = re.search(r'"([^"\s]+)', line)
                    if dep_match:
                        dep_name = dep_match.group(1)
                        if dep_name not in windows_package_names:
                            filtered_deps.append(line)

                # Reconstruct dependencies
                if filtered_deps:
                    new_deps = deps_match.group(1) + '\n  ' + ',\n  '.join(filtered_deps) + ',\n' + deps_match.group(3)
                else:
                    new_deps = 'dependencies = []'

                block = block[:deps_match.start()] + new_deps + block[deps_match.end():]

            final_blocks.append(block)

        # Reconstruct Cargo.lock
        print(header, end="")
        for block in final_blocks:
            print('\n[[package]]' + block, end="")
      '';
    in
      pkgs.runCommand "Cargo.lock-filtered" {} ''
        ${filterScript} ${lockFile} > $out
      '';
in
{
  # Override vendorCargoDeps to skip Windows packages during vendoring
  # Uses crane's overrideVendorCargoPackage hook to intercept each package
  vendorCargoDeps = args:
    cranePrev.vendorCargoDeps (args // {
      # Override individual package downloads
      overrideVendorCargoPackage = pkg: drv:
        if isWindowsPackage pkg.name then
          # Return a stub derivation that creates a minimal valid Rust crate
          # This satisfies cargo's validation without downloading Windows packages
          pkgs.runCommandLocal "stub-${pkg.name}-${pkg.version}" {} ''
            mkdir -p $out/src

            # Create a minimal Cargo.toml that declares the package
            cat > $out/Cargo.toml <<'EOF'
            [package]
            name = "${pkg.name}"
            version = "${pkg.version}"
            edition = "2021"

            [lib]
            path = "src/lib.rs"
            EOF

            # Replace template variables
            sed -i "s/\${pkg.name}/${pkg.name}/g" $out/Cargo.toml
            sed -i "s/\${pkg.version}/${pkg.version}/g" $out/Cargo.toml

            # Create an empty library source file
            cat > $out/src/lib.rs <<EOF
            // Stub implementation for ${pkg.name}
            // This is a Windows-only crate excluded from Linux builds
            EOF

            # Create a valid .cargo-checksum.json
            # Cargo checks this file to verify package integrity
            cat > $out/.cargo-checksum.json <<EOF
            {"files":{},"package":null}
            EOF
          ''
        else
          # Keep original derivation for non-Windows packages
          drv;
    });
}
