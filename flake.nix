{
  description = "faster shell navigation of projects";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs";
  inputs.systems.url = "github:nix-systems/default";

  outputs = { self, nixpkgs, systems }:
    let
      eachSystem = nixpkgs.lib.genAttrs (import systems);
    in
    {
      packages = eachSystem (system:
        let pkgs = nixpkgs.legacyPackages.${system}; in {
          default = pkgs.stdenv.mkDerivation {
            name = "h";
            src = ./.;
            buildInputs = [ pkgs.curl pkgs.cjson ];
            nativeBuildInputs = [ pkgs.pkg-config ];
            makeFlags = [ "PREFIX=$(out)" ];
          };
        });
    };
}
