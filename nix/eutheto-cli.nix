{
  pkgs,
  src,
  ortools-worker ? null,
}:
pkgs.rustPlatform.buildRustPackage {
  pname = "eutheto-cli";
  version = "0.1.0";

  inherit src;
  cargoLock.lockFile = src + "/Cargo.lock";

  cargoBuildFlags = [
    "--package"
    "eutheto-cli"
  ];
  # Integration fixtures require the debug-only pack; the installed build stays release.
  checkType = "debug";
  cargoTestFlags = [
    "--package"
    "eutheto-cli"
  ];

  strictDeps = true;

  preBuild = pkgs.lib.optionalString (ortools-worker != null) ''
    export EUTHETO_ORTOOLS_MANIFEST_PATH=${ortools-worker}/solver-manifest.json
    test -f "$EUTHETO_ORTOOLS_MANIFEST_PATH"
    test ! -L "$EUTHETO_ORTOOLS_MANIFEST_PATH"
    manifestBytes=$(wc -c < "$EUTHETO_ORTOOLS_MANIFEST_PATH")
    test "$manifestBytes" -gt 0
    test "$manifestBytes" -le 65536
    manifestDigest=$(sha256sum "$EUTHETO_ORTOOLS_MANIFEST_PATH")
    export EUTHETO_ORTOOLS_MANIFEST_SHA256="''${manifestDigest%% *}"
  '';

  postInstall = ''
    mkdir -p "$out/libexec/eutheto-cli"
    mv "$out/bin/optimizer" "$out/libexec/eutheto-cli/optimizer"
    ln -s ../libexec/eutheto-cli/optimizer "$out/bin/optimizer"
  '';

  # These bytes are already finalized and manifest-bound. Install them after fixup so
  # stripping, RPATH rewriting and Darwin signing cannot mutate the approved payload.
  postFixup = pkgs.lib.optionalString (ortools-worker != null) ''
    image="$out/libexec/eutheto-cli"
    mkdir -p "$image/solver/ortools"
    cp -R ${ortools-worker}/. "$image/solver/ortools/"
    chmod u+w "$image/solver/ortools" "$image/solver/ortools/bin"
    mv "$image/solver/ortools/bin/ortools-worker" "$image/ortools-worker"
    rmdir "$image/solver/ortools/bin"
    cmp ${ortools-worker}/solver-manifest.json "$image/solver/ortools/solver-manifest.json"
    cmp ${ortools-worker}/bin/ortools-worker "$image/ortools-worker"
  '';

  meta = {
    description = "Eutheto workforce optimization command-line interface";
    license = pkgs.lib.licenses.asl20;
    mainProgram = "optimizer";
    platforms = pkgs.lib.platforms.all;
  };
}
