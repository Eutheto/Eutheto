{
  pkgs,
  src,
  tooling,
}:
let
  rustPlatform = pkgs.makeRustPlatform {
    cargo = tooling.rust;
    rustc = tooling.rust;
  };
  packagePkgs = pkgs // {
    inherit rustPlatform;
  };
  eutheto-cli = import ./eutheto-cli.nix {
    pkgs = packagePkgs;
    inherit src;
    ortools-worker = if supportsOrtoolsWorker then ortools-worker else null;
    pdf-worker = if supportsOrtoolsWorker then pdf-worker else null;
  };
  solver-artifact-tool = import ./solver-artifact-tool.nix {
    pkgs = packagePkgs;
    inherit src;
    protobuf = tooling.protobuf;
  };
  supportsOrtoolsWorker = pkgs.stdenv.hostPlatform.system != "aarch64-linux";
  ortools-worker = import ./ortools-worker.nix {
    inherit pkgs solver-artifact-tool src;
  };
  pdf-worker = import ./pdf-worker.nix {
    pkgs = packagePkgs;
    inherit src solver-artifact-tool;
  };
in
{
  default = eutheto-cli;
  inherit eutheto-cli;
}
// pkgs.lib.optionalAttrs supportsOrtoolsWorker {
  inherit ortools-worker pdf-worker;
}
// pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
  desktop-runtime = tooling.desktopRuntime;
}
