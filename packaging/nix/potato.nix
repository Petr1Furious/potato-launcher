{
  lib,
  rustPlatform,
  makeWrapper,
  git,
  git-lfs,
  rsync,
  openssh,
}:
rustPlatform.buildRustPackage {
  name = "potato";
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
  nativeCheckInputs = [
    git
    git-lfs
  ];
  postFixup = ''
    wrapProgram $out/bin/potato \
      --prefix PATH : "${
        lib.makeBinPath [
          git
          git-lfs
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
