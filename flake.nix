{
  description = "faster shell navigation of projects";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.stdenv.mkDerivation {
          name = "h";
          src = ./.;
          buildInputs = [ pkgs.curl pkgs.cjson ];
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildPhase = ''
            $CC -O2 -Wall $(pkg-config --cflags libcurl libcjson) -o h h.c $(pkg-config --libs libcurl libcjson)
            $CC -O2 -Wall -o up up.c
          '';
          installPhase = ''
            mkdir -p $out/bin
            cp h up $out/bin
          '';
        };
      });
    };
}
