{
  inputs = {
    nixpkgs.url = "https://channels.nixos.org/nixpkgs-unstable/nixexprs.tar.xz";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        packages.default = pkgs.callPackage ./packaging/nix/package.nix { };
        devShells.default = pkgs.mkShell {
          # keep in sync with deps in package.nix
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              xorg.libX11
              xorg.libXext
              xorg.libXcursor
              xorg.libXrandr
              xorg.libXxf86vm
              xorg.libXrender
              xorg.libXtst
              xorg.libXi
              xorg.xrandr
              libpulseaudio
              libGL
              glfw3-minecraft
              openal
              wayland
              libxkbcommon
            ]
          );

          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";

          packages = with pkgs; [
            cargo
            rustc
            clippy

            (python3.withPackages (
              ps: with ps; [
                # flatpak-cargo-generator.py
                aiohttp
                toml
                # configure.py
                tomlkit
                httpx
              ]
            ))
            flatpak-builder
          ];
        };
        formatter = pkgs.nixfmt-tree;
      }
    );
}
