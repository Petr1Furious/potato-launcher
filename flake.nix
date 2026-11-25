{
  inputs = {
    nixpkgs.url = "https://channels.nixos.org/nixpkgs-unstable/nixexprs.tar.xz";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
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
        toolchain = pkgs.fenix.fromToolchainFile {
          file = ./rust-toolchain.toml;
          sha256 = "sha256-MFHoWVcPZKjFrLnQnodCXbUsfZcofGfo9tV9fTTvklg=";
        };
      in
      {
        packages.default = pkgs.callPackage ./packaging/nix/package.nix { };
        packages.potato = pkgs.callPackage ./packaging/nix/potato.nix { };
        apps.potato = {
          type = "app";
          program = "${self.packages.${system}.potato}/bin/potato";
        };
        devShells.default = pkgs.mkShell (
          {
            RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";

            buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (
              with pkgs;
              [
                libxcb
                libxkbcommon
                fontconfig
              ]
            );
            packages =
              with pkgs;
              [
                toolchain
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
              ]
              ++ lib.optionals stdenv.hostPlatform.isLinux [ flatpak-builder ];
          }
          // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
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
          }
        );
        formatter = pkgs.nixfmt-tree;
      }
    );
}
