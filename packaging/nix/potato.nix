{
  lib,
  rustPlatform,
  makeWrapper,
  git,
  rsync,
  openssh,
}:
rustPlatform.buildRustPackage {
  pname = "potato";
  version = "0.1.0";
  src = ./../..;
  cargoLock.lockFile = ../../Cargo.lock;
  cargoBuildFlags = [
    "-p"
    "potato"
  ];
  cargoTestFlags = [
    "-p"
    "potato"
  ];
  nativeBuildInputs = [ makeWrapper ];
  nativeCheckInputs = [ git ];
  postFixup = ''
    wrapProgram $out/bin/potato \
      --prefix PATH : "${
        lib.makeBinPath [
          git
          rsync
          openssh
        ]
      }"
  '';

  meta = {
    description = "Builds and publishes Potato Launcher instances";
    license = lib.licenses.mit;
    mainProgram = "potato";
  };
}
