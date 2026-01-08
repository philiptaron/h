{
  description = "faster shell navigation of projects";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs";
  inputs.systems.url = "github:nix-systems/default";

  outputs =
    inputs:
    let
      eachSystem = inputs.nixpkgs.lib.genAttrs (import inputs.systems);
      version = inputs.self.shortRev or inputs.self.dirtyShortRev or "unknown";
      drv =
        {
          stdenv,
          curl,
          cjson,
          pkg-config,
        }:
        stdenv.mkDerivation {
          name = "h";
          src = ./src;
          buildInputs = [
            curl
            cjson
          ];
          nativeBuildInputs = [ pkg-config ];
          makeFlags = [
            "PREFIX=$(out)"
            "VERSION=${version}"
          ];
        };
    in
    {
      packages = eachSystem (system: {
        default = inputs.nixpkgs.legacyPackages.${system}.callPackage drv { };
      });
    };
}
