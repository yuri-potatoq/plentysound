{ stdenv, lib, package, libvosk ? null, enableTranscriber ? false }:

let
  pname = if enableTranscriber then "plentysound-full" else "plentysound";
  version = "0.1.0";

  baseDeps = "'pipewire' 'dbus'";
  transcriberDeps = "'tar' 'zstd'";
  depends = if enableTranscriber then "${baseDeps} ${transcriberDeps}" else baseDeps;

  sources = if enableTranscriber
    then ''"plentysound" "libvosk.so"''
    else ''"plentysound"'';

  packageCmds = lib.concatStringsSep "\n" ([
    ''install -Dm755 "$srcdir/plentysound" "$pkgdir/usr/bin/plentysound"''
  ] ++ lib.optionals enableTranscriber [
    ''install -Dm755 "$srcdir/libvosk.so" "$pkgdir/usr/lib/libvosk.so"''
  ]);
in
stdenv.mkDerivation {
  pname = "${pname}-aur";
  inherit version;

  dontUnpack = true;

  installPhase = ''
    mkdir -p $out

    cp ${package}/bin/plentysound $out/plentysound
    ${lib.optionalString enableTranscriber ''
      cp ${libvosk}/lib/libvosk.so $out/libvosk.so
    ''}

    cat > $out/PKGBUILD <<'EOF'
    # Maintainer: yuri-potatoq
    pkgname=${pname}
    pkgver=${version}
    pkgrel=1
    pkgdesc="A Linux soundboard that plays audio through PipeWire"
    arch=('x86_64')
    url="https://github.com/yuri-potatoq/plentysound"
    license=('MIT')
    depends=(${depends})
    source=(${sources})
    noextract=(${sources})

    package() {
      ${packageCmds}
    }
    EOF
  '';
}
