{
  description = "faster shell navigation of projects";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs";
  inputs.systems.url = "github:nix-systems/default";

  outputs =
    inputs:
    let
      eachSystem =
        f:
        inputs.nixpkgs.lib.genAttrs (import inputs.systems) (
          system: f inputs.nixpkgs.legacyPackages.${system}
        );
      drv =
        {
          lib,
          rustPlatform,
          bashInteractive,
          git,
          zsh,
        }:
        rustPlatform.buildRustPackage {
          pname = "h";
          version = (lib.importTOML ./Cargo.toml).workspace.package.version;
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./crates
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          nativeCheckInputs = [
            bashInteractive
            git
            zsh
          ];
        };
    in
    {
      packages = eachSystem (pkgs: {
        default = pkgs.callPackage drv { };
      });
      devShells = eachSystem (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ (pkgs.callPackage drv { }) ];
          packages = with pkgs; [
            clippy
            rustfmt
            rust-analyzer
          ];
        };
      });
    };
}
