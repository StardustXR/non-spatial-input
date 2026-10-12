{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
    }:
    let
      supportedSystems = [
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      nixpkgsFor = forAllSystems (system: import nixpkgs { inherit system; });
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgsFor.${system};
          craneLib = crane.mkLib pkgs;
        in
        {
          default = craneLib.buildPackage {
            src = ./.;
            cargoLock = ./Cargo.lock;
            doCheck = false;
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
              wayland
            ];

            nativeBuildInputs = [ pkgs.just ];
            postInstall = "just prefix=$out install-data";
            STARDUST_RES_PREFIXES = "${builtins.placeholder "out"}/share";
          };
        }
      );
      formatter = forAllSystems (system: nixpkgsFor.${system}.nixfmt-tree);

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgsFor.${system};
        in
        {
          default = pkgs.mkShell rec {
            nativeBuildInputs = with pkgs; [
              cargo
              rustc
              just
            ];
            buildInputs = (
              with pkgs;
              [
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
                wayland
              ]
            );
            LD_LIBRARY_PATH = "${nixpkgs.lib.makeLibraryPath buildInputs}";
          };
        }
      );
    };
}
