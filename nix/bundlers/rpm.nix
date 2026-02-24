{ stdenv, writeText, rpm, lib, package, libvosk ? null, enableTranscriber ? false }:

let
  pname = if enableTranscriber then "plentysound-full" else "plentysound";
  version = "0.1.0";
  release = "1";

  baseRequires = "pipewire dbus";
  transcriberRequires = "tar zstd";
  requires = if enableTranscriber then "${baseRequires} ${transcriberRequires}" else baseRequires;

  specFile = writeText "${pname}.spec" ''
    Name:           ${pname}
    Version:        ${version}
    Release:        ${release}
    Summary:        A Linux soundboard that plays audio through PipeWire
    License:        MIT
    URL:            https://github.com/yuri-potatoq/plentysound
    Requires:       ${requires}

    %description
    A Linux soundboard that plays audio through PipeWire, letting you send
    sounds into Discord, browsers, or any application that accepts audio input.

    %install
    mkdir -p %{buildroot}/usr/bin
    cp ${package}/bin/plentysound %{buildroot}/usr/bin/plentysound
    ${lib.optionalString enableTranscriber ''
    mkdir -p %{buildroot}/usr/lib64
    cp ${libvosk}/lib/libvosk.so %{buildroot}/usr/lib64/libvosk.so
    ''}

    %files
    /usr/bin/plentysound
    ${lib.optionalString enableTranscriber "/usr/lib64/libvosk.so"}
  '';
in
stdenv.mkDerivation {
  pname = "${pname}-rpm";
  inherit version;

  nativeBuildInputs = [ rpm ];

  dontUnpack = true;

  buildPhase = ''
    topdir=$(pwd)/rpmbuild
    mkdir -p $topdir/{BUILD,RPMS,SOURCES,SPECS,SRPMS,BUILDROOT,tmp,rpmdb}

    rpmbuild -bb \
      --define "_topdir $topdir" \
      --define "_dbpath $topdir/rpmdb" \
      --define "_tmppath $topdir/tmp" \
      --define "_rpmdir $topdir/RPMS" \
      --buildroot "$topdir/BUILDROOT" \
      ${specFile}
  '';

  installPhase = ''
    find rpmbuild/RPMS -name '*.rpm' -exec cp {} $out \;
  '';
}
