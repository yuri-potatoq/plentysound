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

  # Create minimal stub derivation for a Windows package
  # Only creates the minimum files needed for cargo to validate the vendor directory
  createMinimalStub = { name, version, ... }:
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
      default = []
      EOF

      echo "// Minimal stub for Windows-only crate ${name}" > $out/src/lib.rs

      echo '{"files":{},"package":null}' > $out/.cargo-checksum.json
    '';

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
            lines = f.readlines()

        # Patterns for Windows packages
        windowsPatternsRegex = "${windowsPatternsRegex}"
        windows_patterns = re.compile(r'^name = "(' + windowsPatternsRegex + r')"')

        # First pass: identify Windows packages
        windows_package_names = set()
        i = 0
        while i < len(lines):
            line = lines[i]
            if line.startswith('[[package]]'):
                # Check if this is a Windows package
                j = i + 1
                pkg_name = None
                while j < len(lines) and not lines[j].startswith('[[package]]'):
                    if lines[j].startswith('name = '):
                        name_match = re.match(r'name = "([^"]+)"', lines[j])
                        if name_match:
                            pkg_name = name_match.group(1)
                            break
                    j += 1
                if pkg_name and windows_patterns.match('name = "' + pkg_name + '"'):
                    windows_package_names.add(pkg_name)
            i += 1

        # Second pass: filter out Windows packages and their dependencies
        output_lines = []
        i = 0
        while i < len(lines):
            line = lines[i]

            if line.startswith('[[package]]'):
                # Check if this package should be filtered
                j = i + 1
                pkg_name = None
                # Scan forward to find package name and next [[package]]
                while j < len(lines) and not lines[j].startswith('[[package]]'):
                    if pkg_name is None and lines[j].startswith('name = '):
                        name_match = re.match(r'name = "([^"]+)"', lines[j])
                        if name_match:
                            pkg_name = name_match.group(1)
                            # Don't break - continue to find next [[package]]
                    j += 1

                if pkg_name and pkg_name in windows_package_names:
                    # Skip this entire package block (j now points to next [[package]] or EOF)
                    i = j
                    continue
                else:
                    # Process this package block
                    output_lines.append(line)
                    i += 1

                    # Process package content, filtering dependencies
                    in_dependencies = False
                    while i < len(lines) and not lines[i].startswith('[[package]]'):
                        if lines[i].startswith('dependencies = ['):
                            in_dependencies = True
                            output_lines.append(lines[i])
                            i += 1

                            # Filter dependency lines
                            while i < len(lines) and not lines[i].strip() == ']':
                                dep_line = lines[i]
                                # Extract dependency name (line format: ' "dep_name version",\n')
                                dep_match = re.match(r'\s*"([^\s"]+)', dep_line)
                                if dep_match:
                                    dep_name = dep_match.group(1)
                                    if dep_name not in windows_package_names:
                                        output_lines.append(dep_line)
                                    # else: skip this Windows dependency
                                else:
                                    # Not a dependency line, keep it
                                    output_lines.append(dep_line)
                                i += 1

                            # Add closing bracket
                            if i < len(lines):
                                output_lines.append(lines[i])
                                i += 1
                            in_dependencies = False
                        else:
                            output_lines.append(lines[i])
                            i += 1
            else:
                output_lines.append(line)
                i += 1

        # Write output
        sys.stdout.write('''.join(output_lines))
      '';
    in
      pkgs.runCommand "Cargo.lock-filtered" {} ''
        ${filterScript} ${lockFile} > $out
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

  # Override vendorCargoDeps to filter Windows crates BEFORE vendoring
  # This prevents downloading ~180MB of Windows packages entirely
  vendorCargoDeps = args:
    let
      # Reuse existing filterCargoLock function to remove Windows deps
      filteredLock = filterCargoLock { inherit (args) src; };
    in
      # Pass filtered Cargo.lock to crane - prevents downloading Windows packages!
      cranePrev.vendorCargoDeps (args // {
        cargoLock = filteredLock;
      });

  # Override buildDepsOnly to use filtered Cargo.lock
  # This ensures cargo uses the filtered lock during dependency builds
  buildDepsOnly = args:
    let
      filteredLock = filterCargoLock { inherit (args) src; };
    in
      cranePrev.buildDepsOnly (args // {
        cargoLock = filteredLock;
      });

  # Override buildPackage to use filtered Cargo.lock
  # This ensures cargo uses the filtered lock during the final build
  buildPackage = args:
    let
      filteredLock = filterCargoLock { inherit (args) src; };
    in
      cranePrev.buildPackage (args // {
        cargoLock = filteredLock;
      });
}
