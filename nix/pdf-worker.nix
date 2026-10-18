{
  pkgs,
  src,
  solver-artifact-tool,
}:
let
  targetTriple =
    {
      "x86_64-linux" = "x86_64-unknown-linux-gnu";
      "x86_64-darwin" = "x86_64-apple-darwin";
      "aarch64-darwin" = "aarch64-apple-darwin";
    }
    .${pkgs.stdenv.hostPlatform.system};
in
pkgs.rustPlatform.buildRustPackage {
  pname = "eutheto-pdf-worker";
  version = "0.1.0";
  inherit src;
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [
    "--package"
    "eutheto-pdf-worker"
  ];
  cargoTestFlags = [
    "--package"
    "eutheto-pdf-worker"
  ];
  strictDeps = true;

  postInstall = ''
    cp crates/eutheto-pdf-worker/fonts/UPSTREAM-NOTICE "$out/UPSTREAM-NOTICE"
    cp crates/eutheto-pdf-worker/fonts/UPSTREAM-LICENSE "$out/UPSTREAM-LICENSE"
    cp crates/eutheto-pdf-worker/fonts/COMPACT-ICC-LICENSE "$out/COMPACT-ICC-LICENSE"
    cp crates/eutheto-pdf-worker/fonts/ICU4X-LICENSE "$out/ICU4X-LICENSE"
    cp crates/eutheto-pdf-worker/fonts/W3C-DOCUMENT-LICENSE-2015 "$out/W3C-DOCUMENT-LICENSE-2015"
    cp crates/eutheto-pdf-worker/fonts/ASSET-PROVENANCE-NOTICE "$out/ASSET-PROVENANCE-NOTICE"
  '';

  # Bind the installed bytes only after stripping, RPATH fixups and Darwin signing.
  postFixup = ''
    ${solver-artifact-tool}/bin/xtask pdf finalize \
      --authority-root ${src} \
      --executable "$out/bin/eutheto-pdf-worker" \
      --manifest "$out/eutheto-pdf-worker.json" \
      --target-triple ${targetTriple}
  '';

  meta = {
    description = "Eutheto fixed offline recipient PDF renderer";
    license = pkgs.lib.licenses.asl20;
    mainProgram = "eutheto-pdf-worker";
    platforms = [
      "x86_64-linux"
      "x86_64-darwin"
      "aarch64-darwin"
    ];
  };
}
