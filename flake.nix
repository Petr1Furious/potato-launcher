{
  inputs = {
    nixpkgs.url = "https://channels.nixos.org/nixpkgs-unstable/nixexprs.tar.xz";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      fenix,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ fenix.overlays.default ];
        };
      in
      {
        packages.default = pkgs.callPackage ./packaging/nix/package.nix { };
        devShells.default = pkgs.mkShell {
          # keep in sync with deps in package.nix
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              libx11
              libxext
              libxcursor
              libxrandr
              libxxf86vm
              libxrender
              libxtst
              libxi
              xrandr
              libxkbcommon
              libpulseaudio
              libGL
              glfw3-minecraft
              openal
              wayland
              vulkan-loader
              libxcb
            ]
          );

          RUST_SRC_PATH = "${pkgs.fenix.complete.rust-src}/lib/rustlib/src/rust/library";

          buildInputs = with pkgs; [
            libxcb
            libxkbcommon
            fontconfig
          ];
          packages = with pkgs; [
            # nightly toolchain
            pkgs.fenix.complete.toolchain
            pkg-config

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
