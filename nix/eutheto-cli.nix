{
  pkgs,
  src,
  ortools-worker ? null,
  pdf-worker ? null,
}:
pkgs.rustPlatform.buildRustPackage {
  pname = "eutheto-cli";
  version = "0.1.0";

  inherit src;
  # --no-build cannot materialize the filtered source during lockfile evaluation.
  cargoLock.lockFile = ../Cargo.lock;

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

  preBuild =
    pkgs.lib.optionalString (ortools-worker != null) ''
      export EUTHETO_ORTOOLS_MANIFEST_PATH=${ortools-worker}/solver-manifest.json
      test -f "$EUTHETO_ORTOOLS_MANIFEST_PATH"
      test ! -L "$EUTHETO_ORTOOLS_MANIFEST_PATH"
      manifestBytes=$(wc -c < "$EUTHETO_ORTOOLS_MANIFEST_PATH")
      test "$manifestBytes" -gt 0
      test "$manifestBytes" -le 65536
      manifestDigest=$(sha256sum "$EUTHETO_ORTOOLS_MANIFEST_PATH")
      export EUTHETO_ORTOOLS_MANIFEST_SHA256="''${manifestDigest%% *}"
    ''
    + pkgs.lib.optionalString (pdf-worker != null) ''
      export EUTHETO_PDF_MANIFEST_PATH=${pdf-worker}/eutheto-pdf-worker.json
      test -f "$EUTHETO_PDF_MANIFEST_PATH"
      test ! -L "$EUTHETO_PDF_MANIFEST_PATH"
      pdfManifestBytes=$(wc -c < "$EUTHETO_PDF_MANIFEST_PATH")
      test "$pdfManifestBytes" -gt 0
      test "$pdfManifestBytes" -le 4096
      pdfManifestDigest=$(sha256sum "$EUTHETO_PDF_MANIFEST_PATH")
      export EUTHETO_PDF_MANIFEST_SHA256="''${pdfManifestDigest%% *}"
    '';

  postInstall = ''
    mkdir -p "$out/libexec/eutheto-cli"
    mv "$out/bin/optimizer" "$out/libexec/eutheto-cli/optimizer"
    ln -s ../libexec/eutheto-cli/optimizer "$out/bin/optimizer"
  '';

  # These bytes are already finalized and manifest-bound. Install them after fixup so
  # stripping, RPATH rewriting and Darwin signing cannot mutate the approved payload.
  postFixup =
    pkgs.lib.optionalString (ortools-worker != null) ''
      image="$out/libexec/eutheto-cli"
      mkdir -p "$image/solver/ortools"
      cp -R ${ortools-worker}/. "$image/solver/ortools/"
      chmod u+w "$image/solver/ortools" "$image/solver/ortools/bin"
      mv "$image/solver/ortools/bin/ortools-worker" "$image/ortools-worker"
      rmdir "$image/solver/ortools/bin"
      cmp ${ortools-worker}/solver-manifest.json "$image/solver/ortools/solver-manifest.json"
      cmp ${ortools-worker}/bin/ortools-worker "$image/ortools-worker"
    ''
    + pkgs.lib.optionalString (pdf-worker != null) ''
      image="$out/libexec/eutheto-cli"
      mkdir -p "$image/renderer/pdf"
      cp ${pdf-worker}/bin/eutheto-pdf-worker "$image/eutheto-pdf-worker"
      cp ${pdf-worker}/eutheto-pdf-worker.json "$image/eutheto-pdf-worker.json"
      cp ${pdf-worker}/UPSTREAM-NOTICE "$image/renderer/pdf/UPSTREAM-NOTICE"
      cp ${pdf-worker}/UPSTREAM-LICENSE "$image/renderer/pdf/UPSTREAM-LICENSE"
      cp ${pdf-worker}/COMPACT-ICC-LICENSE "$image/renderer/pdf/COMPACT-ICC-LICENSE"
      cp ${pdf-worker}/ICU4X-LICENSE "$image/renderer/pdf/ICU4X-LICENSE"
      cp ${pdf-worker}/W3C-DOCUMENT-LICENSE-2015 "$image/renderer/pdf/W3C-DOCUMENT-LICENSE-2015"
      cp ${pdf-worker}/ASSET-PROVENANCE-NOTICE "$image/renderer/pdf/ASSET-PROVENANCE-NOTICE"
      cmp ${pdf-worker}/bin/eutheto-pdf-worker "$image/eutheto-pdf-worker"
      cmp ${pdf-worker}/eutheto-pdf-worker.json "$image/eutheto-pdf-worker.json"
      cmp ${pdf-worker}/UPSTREAM-NOTICE "$image/renderer/pdf/UPSTREAM-NOTICE"
      cmp ${pdf-worker}/UPSTREAM-LICENSE "$image/renderer/pdf/UPSTREAM-LICENSE"
      cmp ${pdf-worker}/COMPACT-ICC-LICENSE "$image/renderer/pdf/COMPACT-ICC-LICENSE"
      cmp ${pdf-worker}/ICU4X-LICENSE "$image/renderer/pdf/ICU4X-LICENSE"
      cmp ${pdf-worker}/W3C-DOCUMENT-LICENSE-2015 "$image/renderer/pdf/W3C-DOCUMENT-LICENSE-2015"
      cmp ${pdf-worker}/ASSET-PROVENANCE-NOTICE "$image/renderer/pdf/ASSET-PROVENANCE-NOTICE"
    '';

  meta = {
    description = "Eutheto workforce optimization command-line interface";
    license = pkgs.lib.licenses.asl20;
    mainProgram = "optimizer";
    platforms = pkgs.lib.platforms.all;
  };
}
