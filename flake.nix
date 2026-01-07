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
          buildInputs = [ pkgs.ruby ];
          installPhase = ''
            mkdir -p $out/bin
            cp h $out/bin
            cp up $out/bin
          '';
        };
      });
    };
}
