{
  lib,
  rustPlatform,
  makeWrapper,
  libx11,
  libxext,
  libxcursor,
  libxrandr,
  libxxf86vm,
  libxrender,
  libxtst,
  libxi,
  xrandr,
  libxkbcommon,
  libpulseaudio,
  libGL,
  glfw3-minecraft,
  openal,
  wayland,
  vulkan-loader,
  version ? null,
}:
let
  loadDotenv = import ./loadDotenv.nix { inherit lib; };
  env = loadDotenv ../../build.env;
  # keep in sync with deps in flake.nix
  runtimeDeps = [
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
  ];
in
rustPlatform.buildRustPackage {
  name = lib.pipe env.LAUNCHER_NAME [
    lib.toLower
    (lib.replaceStrings [ " " ] [ "-" ])
    (lib.replaceStrings [ "'" ] [ "" ])
  ];
  src = ./../..;
  cargoLock.lockFile = ../../Cargo.lock;
  cargoBuildFlags = [
    "-p"
    "launcher"
  ];
  cargoTestFlags = [
    "-p"
    "launcher"
  ];

  USE_NATIVE_GLFW_DEFAULT = "true";
  VERSION = if version == null then "" else version;
  nativeBuildInputs = [ makeWrapper ];
  postFixup = ''
    wrapProgram $out/bin/launcher \
      --set LD_LIBRARY_PATH "${lib.makeLibraryPath runtimeDeps}"
  '';

  meta = {
    description = env.LAUNCHER_DESCRIPTION or null;
    license = lib.licenses.mit;
    mainProgram = "launcher";
    platforms = lib.platforms.linux;
  };
}
