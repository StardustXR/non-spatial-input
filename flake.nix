{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane = {
      inputs.nixpkgs.follows = "nixpkgs";
      url = "github:ipetkov/crane";
    };
  };


  outputs = { self, nixpkgs, crane }:
  let supportedSystems = [ "aarch64-linux" "x86_64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      nixpkgsFor = forAllSystems (system: import nixpkgs { inherit system; });
  in {
    packages = forAllSystems (system: let pkgs = nixpkgsFor.${system}; in {
      default = crane.lib.${system}.buildPackage {
        pname = "non-spatial-input";
        version = "0.1.0";
        src = ./.;
        buildInputs = with pkgs; [
          # inputs
          pkg-config
          libxkbcommon
          
          # eclipse
          libinput
          
          # manifold
          libGL
          xorg.libX11
          xorg.libXcursor
          xorg.libXrandr
          xorg.libXi
        ];
      };
    });

    devShells = forAllSystems (system: let pkgs = nixpkgsFor.${system}; in {
      default = pkgs.mkShell {
        buildInputs = (with pkgs; [
          # inputs
          pkg-config
          libxkbcommon
          
          # eclipse
          libinput
          
          # manifold
          libGL
          xorg.libX11
          xorg.libXcursor
          xorg.libXrandr
          xorg.libXi
        ]);
      };
    });
  };
}
