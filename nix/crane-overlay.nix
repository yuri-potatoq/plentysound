# Crane overlay to prevent downloading Windows-specific cargo packages
# Uses lightweight stub packages instead of downloading ~180MB from crates.io
#
# How it works:
#   - Overrides downloadCargoPackage in crane's scope
#   - Returns feature-complete stubs for Windows packages
#   - Real packages downloaded for all other platforms
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

  # Create feature-complete stub derivation for a Windows package
  # This includes all common Windows crate features to satisfy Cargo's dependency resolution
  # while providing empty implementations that are never compiled (Cargo.lock is filtered)
  createFeatureCompleteStub = { name, version, checksum ? null, ... }:
    let
      # Comprehensive list of common Windows crate features
      # Covers winapi, windows-sys, windows-targets, and other Windows crates
      commonFeatures = [
        # Basic features
        "default" "std" "debug" "nightly"

        # winapi common features
        "winerror" "winuser" "winbase" "winnt" "wincon" "consoleapi"
        "handleapi" "processenv" "synchapi" "fileapi" "libloaderapi"
        "errhandlingapi" "winreg" "impl-default" "impl-debug"
        "minwindef" "basetsd" "guiddef" "minwinbase" "vadefs"
        "windef" "ntdef" "ntstatus" "winsock2" "ws2def"
        "ws2ipdef" "ws2tcpip" "mswsock" "winioctl" "ioapiset"
        "namedpipeapi" "profileapi" "psapi" "winperf" "winsvc"
        "wow64apiset" "timezoneapi" "timeapi" "shellapi" "shlobj"
        "combaseapi" "objbase" "cguid" "wtypesbase" "unknwnbase"
        "objidl" "propidl" "oleauto" "oleidl" "servprov"
        "wincrypt" "dpapi" "securitybaseapi" "winnetwk" "lmcons"
        "heapapi" "memoryapi" "winhttp" "wininet" "urlmon"

        # windows-sys features (Win32 APIs) - comprehensive list
        # Core categories
        "Win32" "Win32_Foundation"

        # System features
        "Win32_System" "Win32_System_Threading" "Win32_System_Console"
        "Win32_System_Diagnostics" "Win32_System_Diagnostics_Debug"
        "Win32_System_IO" "Win32_System_Kernel" "Win32_System_Memory"
        "Win32_System_Pipes" "Win32_System_ProcessStatus"
        "Win32_System_SystemInformation" "Win32_System_SystemServices"
        "Win32_System_WindowsProgramming" "Win32_System_Registry"
        "Win32_System_LibraryLoader" "Win32_System_Power"
        "Win32_System_Time" "Win32_System_Com"

        # Security
        "Win32_Security" "Win32_Security_Authentication"
        "Win32_Security_Cryptography"

        # Storage
        "Win32_Storage" "Win32_Storage_FileSystem"

        # Networking
        "Win32_Networking" "Win32_Networking_WinSock"
        "Win32_NetworkManagement" "Win32_NetworkManagement_IpHelper"
        "Win32_NetworkManagement_Ndis"

        # UI
        "Win32_UI" "Win32_UI_Input" "Win32_UI_Input_KeyboardAndMouse"
        "Win32_UI_WindowsAndMessaging" "Win32_UI_Shell"

        # Graphics
        "Win32_Graphics" "Win32_Graphics_Gdi"

        # Globalization
        "Win32_Globalization"

        # Windows Driver Kit (WDK) features - needed by mio and other crates
        "Wdk" "Wdk_Foundation" "Wdk_System" "Wdk_System_Threading"
        "Wdk_System_IO" "Wdk_System_SystemServices"
        "Wdk_Storage" "Wdk_Storage_FileSystem"

        # windows-targets features
        "windows_aarch64_gnullvm" "windows_aarch64_msvc"
        "windows_i686_gnu" "windows_i686_msvc"
        "windows_x86_64_gnu" "windows_x86_64_gnullvm" "windows_x86_64_msvc"

        # Generic catch-all features
        "everything" "all" "link" "implement"
      ];

      # Deduplicate features to avoid "duplicate key" errors in Cargo.toml
      uniqueFeatures = lib.unique commonFeatures;

      # Format features for Cargo.toml
      featureList = lib.concatMapStringsSep "\n" (f: "${f} = []") uniqueFeatures;
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
${featureList}
EOF

      cat > $out/src/lib.rs <<'RUST_EOF'
// Feature-complete stub for Windows-only crate ${name}
// This stub provides all common features to satisfy Cargo's dependency resolution
// but is never compiled on Linux (Cargo.lock is filtered to exclude Windows crates)
//
// All types and functions are no-ops on non-Windows platforms
RUST_EOF

      # Use the checksum from Cargo.lock if provided, otherwise use a dummy value
      ${if checksum != null then ''
        echo '{"files":{},"package":"${checksum}"}' > $out/.cargo-checksum.json
      '' else ''
        echo '{"files":{},"package":"0000000000000000000000000000000000000000000000000000000000000000"}' > $out/.cargo-checksum.json
      ''}
    '';
in
{
  # Override downloadCargoPackage to create stubs for Windows packages
  # This prevents downloading real Windows packages (~180MB) at the source
  downloadCargoPackage = pkg:
    if isWindowsPackage pkg.name then
      createFeatureCompleteStub {
        inherit (pkg) name version checksum;
      }
    else
      cranePrev.downloadCargoPackage pkg;

}
