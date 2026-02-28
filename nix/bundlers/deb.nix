{ stdenv, dpkg, fakeroot, lib, package, libvosk ? null, enableTranscriber ? false, patchelf }:

let
  pname = if enableTranscriber then "plentysound-full" else "plentysound";
  version = "0.1.0";
  arch = "amd64";

  baseDeps = "pipewire, dbus";
  transcriberDeps = "tar, zstd";
  depends = if enableTranscriber then "${baseDeps}, ${transcriberDeps}" else baseDeps;
in
stdenv.mkDerivation {
  pname = "${pname}-deb";
  inherit version;

  nativeBuildInputs = [ dpkg fakeroot patchelf ];

  dontUnpack = true;

  buildPhase = ''
    mkdir -p pkg/DEBIAN
    mkdir -p pkg/usr/bin

    cat > pkg/DEBIAN/control <<EOF
    Package: ${pname}
    Version: ${version}
    Section: sound
    Priority: optional
    Architecture: ${arch}
    Depends: ${depends}
    Maintainer: yuri-potatoq
    Homepage: https://github.com/yuri-potatoq/plentysound
    License: MIT
    Description: A Linux soundboard that plays audio through PipeWire
    EOF

    cp ${package}/bin/plentysound pkg/usr/bin/plentysound
    chmod +w pkg/usr/bin/plentysound
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 \
             --set-rpath /lib/x86_64-linux-gnu:/usr/lib/x86_64-linux-gnu:/usr/lib:/lib:/lib64 \
             pkg/usr/bin/plentysound

    ${lib.optionalString enableTranscriber ''
      mkdir -p pkg/usr/lib
      cp ${libvosk}/lib/libvosk.so pkg/usr/lib/libvosk.so
      chmod +w pkg/usr/lib/libvosk.so
      patchelf --set-rpath /lib/x86_64-linux-gnu:/usr/lib/x86_64-linux-gnu:/usr/lib:/lib:/lib64 pkg/usr/lib/libvosk.so
    ''}
  '';

  installPhase = ''
    fakeroot dpkg-deb --build pkg $out
  '';
}
