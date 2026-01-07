{
  description = "Faster shell navigation of projects";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forEachSystem = f: nixpkgs.lib.genAttrs supportedSystems (system: f {
        inherit system;
        pkgs = nixpkgs.legacyPackages.${system};
      });
    in
    {
      packages = forEachSystem ({ pkgs, ... }: {
        default = pkgs.buildGoModule {
          pname = "h";
          version = "0.1.0";
          src = ./.;
          vendorHash = null;
          subPackages = [ "cmd/h" "cmd/up" ];
          meta.mainProgram = "h";
        };
      });
    };
}
