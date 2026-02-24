{ stdenv, fetchurl, unzip, autoPatchelfHook }:

stdenv.mkDerivation {
  pname = "libvosk";
  version = "0.3.45";

  src = fetchurl {
    url = "https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-linux-x86_64-0.3.45.zip";
    sha256 = "sha256-u9yO2FxDl59kQxQoiXcOqVy/vFbP+1xdzXOvqHXF+7I=";
  };

  nativeBuildInputs = [ unzip autoPatchelfHook ];
  buildInputs = [ stdenv.cc.cc.lib ];

  unpackPhase = "unzip $src";

  installPhase = ''
    mkdir -p $out/lib $out/include
    cp vosk-linux-x86_64-0.3.45/libvosk.so $out/lib/
    cp vosk-linux-x86_64-0.3.45/vosk_api.h $out/include/
  '';
}
