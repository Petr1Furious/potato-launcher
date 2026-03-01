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
  # TODO: auto update this
  cargoHash = "sha256-xR9esX9eLS2q2Ghoynn9KOKm4+JVxgZbUX6GY2+jjz4=";

  USE_NATIVE_GLFW_DEFAULT = "true";
  nativeBuildInputs = [ makeWrapper ];
  postFixup = ''
    wrapProgram $out/bin/launcher \
      --set LD_LIBRARY_PATH "${lib.makeLibraryPath runtimeDeps}"
  '';

  meta = {
    description = env.LAUNCHER_DESCRIPTION or null;
    license = lib.licenses.mit;
    mainProgram = "launcher";
  };
}
