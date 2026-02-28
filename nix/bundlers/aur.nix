{ stdenv, lib, package, libvosk ? null, enableTranscriber ? false, patchelf, fakeroot, zstd, gzip, libarchive }:

let
  pname = if enableTranscriber then "plentysound-full" else "plentysound";
  version = "0.1.0";
  pkgrel = "1";
  arch = "x86_64";

  baseDeps = "pipewire dbus";
  transcriberDeps = "tar zstd";
  depends = if enableTranscriber then "${baseDeps} ${transcriberDeps}" else baseDeps;
in
stdenv.mkDerivation {
  pname = "${pname}-aur";
  inherit version;

  nativeBuildInputs = [ patchelf fakeroot zstd gzip libarchive ];

  dontUnpack = true;

  buildPhase = ''
    # Create package directory structure
    mkdir -p pkg/usr/bin

    # Copy and patch binary
    cp ${package}/bin/plentysound pkg/usr/bin/plentysound
    chmod +w pkg/usr/bin/plentysound
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 \
             --set-rpath /usr/lib \
             pkg/usr/bin/plentysound

    ${lib.optionalString enableTranscriber ''
      mkdir -p pkg/usr/lib
      cp ${libvosk}/lib/libvosk.so pkg/usr/lib/libvosk.so
      chmod +w pkg/usr/lib/libvosk.so
      patchelf --set-rpath /usr/lib pkg/usr/lib/libvosk.so
    ''}

    # Create .PKGINFO file
    cat > pkg/.PKGINFO <<EOF
    pkgname = ${pname}
    pkgver = ${version}-${pkgrel}
    pkgdesc = A Linux soundboard that plays audio through PipeWire
    url = https://github.com/yuri-potatoq/plentysound
    builddate = $(date +%s)
    packager = yuri-potatoq
    size = $(du -sb pkg | cut -f1)
    arch = ${arch}
    license = MIT
    depend = ${lib.replaceStrings [" "] ["\ndepend = "] depends}
    EOF

    # Create .MTREE file
    cd pkg
    bsdtar -czf .MTREE --format=mtree \
      --options='!all,use-set,type,uid,gid,mode,time,size,md5,sha256,link' \
      *
    cd ..

    # Create the package archive
    cd pkg
    fakeroot -- bsdtar -cf - .MTREE .PKGINFO usr | zstd -19 -T0 --ultra > ../${pname}-${version}-${pkgrel}-${arch}.pkg.tar.zst
    cd ..
  '';

  installPhase = ''
    mkdir -p $out
    cp ${pname}-${version}-${pkgrel}-${arch}.pkg.tar.zst $out/
  '';
}
