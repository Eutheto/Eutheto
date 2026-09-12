{
  nixgl,
  nixpkgs,
  pkgs,
  system,
}:
let
  inherit (pkgs) lib;
  nixglPackages = import nixgl {
    inherit pkgs;
    enable32bits = system == "x86_64-linux";
    enableIntelX86Extensions = system == "x86_64-linux";
  };

  rust = pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;
  protobuf = pkgs.protobuf_33;
  nodeVersion = "24.20.0";
  nodeDistributions = {
    x86_64-linux = {
      artifact = "linux-x64";
      sha256 = "2f2c0da162318f0de47665410c7c8c2ed3d36c8f3105de4bbc61176c70a7cbf2";
    };
    aarch64-linux = {
      artifact = "linux-arm64";
      sha256 = "5f4ddab610c1ab2016b3c227cebdbf6d9495161487e4739c7b90090595f465f7";
    };
    x86_64-darwin = {
      artifact = "darwin-x64";
      sha256 = "26fc30891004603d094eed11de5efcd03bbd2efbc35c177fc72648d5d7a7701b";
    };
    aarch64-darwin = {
      artifact = "darwin-arm64";
      sha256 = "b7bf7707070b950ba1ec5f1af3bb6de0f2b1962c5033973d94068ab021ef3014";
    };
  };
  nodeDistribution =
    nodeDistributions.${system} or (throw "unsupported eutheto Node.js development system: ${system}");
  node = pkgs.stdenv.mkDerivation {
    pname = "nodejs";
    version = nodeVersion;

    src = pkgs.fetchurl {
      url = "https://nodejs.org/dist/v${nodeVersion}/node-v${nodeVersion}-${nodeDistribution.artifact}.tar.xz";
      sha256 = nodeDistribution.sha256;
    };

    nativeBuildInputs = [
      pkgs.makeWrapper
    ]
    ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.autoPatchelfHook ];
    buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [
      pkgs.stdenv.cc.cc.lib
      pkgs.stdenv.cc.libc
    ];
    dontBuild = true;

    installPhase = ''
      runHook preInstall

      mkdir -p "$out"
      cp -a ./. "$out/"

      rm "$out/bin/npm" "$out/bin/npx" "$out/bin/corepack"
      makeWrapper "$out/bin/node" "$out/bin/npm" \
        --add-flags "$out/lib/node_modules/npm/bin/npm-cli.js"
      makeWrapper "$out/bin/node" "$out/bin/npx" \
        --add-flags "$out/lib/node_modules/npm/bin/npx-cli.js"
      makeWrapper "$out/bin/node" "$out/bin/corepack" \
        --add-flags "$out/lib/node_modules/corepack/dist/corepack.js"

      runHook postInstall
    '';

    doInstallCheck = true;
    installCheckPhase = ''
      test "$("$out/bin/node" --version)" = "v${nodeVersion}"
      "$out/bin/npm" --version >/dev/null
      "$out/bin/npx" --version >/dev/null
      "$out/bin/corepack" --version >/dev/null
    '';

    meta = {
      description = "Node.js JavaScript runtime";
      homepage = "https://nodejs.org/";
      license = lib.licenses.mit;
      mainProgram = "node";
      platforms = builtins.attrNames nodeDistributions;
    };
  };

  pnpmVersion = "11.24.0";

  pnpmFromRegistry = pkgs.stdenvNoCC.mkDerivation {
    pname = "pnpm";
    version = pnpmVersion;

    src = pkgs.fetchurl {
      url = "https://registry.npmjs.org/pnpm/-/pnpm-${pnpmVersion}.tgz";
      hash = "sha512-vSfjRel23LC+C3oSKCF7BJqBfiGx81XJDb59xGZxiVqLwebQbCRVRQXqk+oLRfSJon7Bv7yN5qlln8oPFvoAAA==";
    };

    sourceRoot = "package";
    nativeBuildInputs = [ pkgs.makeWrapper ];
    dontBuild = true;

    installPhase = ''
      runHook preInstall

      mkdir -p "$out/bin" "$out/lib/node_modules/pnpm"
      cp -R . "$out/lib/node_modules/pnpm"
      makeWrapper ${node}/bin/node "$out/bin/pnpm" \
        --add-flags "$out/lib/node_modules/pnpm/bin/pnpm.cjs"
      makeWrapper ${node}/bin/node "$out/bin/pnpx" \
        --add-flags "$out/lib/node_modules/pnpm/bin/pnpx.cjs"

      runHook postInstall
    '';

    meta = {
      description = "Fast, disk space efficient package manager";
      homepage = "https://pnpm.io/";
      license = lib.licenses.mit;
      mainProgram = "pnpm";
      platforms = lib.platforms.unix;
    };
  };
  pnpm = pnpmFromRegistry;

  tauriDriverVersion = "2.0.6";
  tauriDriver = pkgs.rustPlatform.buildRustPackage {
    pname = "tauri-driver";
    version = tauriDriverVersion;

    src = pkgs.fetchCrate {
      pname = "tauri-driver";
      version = tauriDriverVersion;
      hash = "sha256-fTCkEs4NLBW0khaHL4jpVNkrbQg22YPsRMjfJNqnCWA=";
    };
    cargoHash = "sha256-MThAcU+U8PyBGauh3dy7ZRvRX9INmOEeghIlQEGLAPs=";

    meta = {
      description = "WebDriver intermediary for Tauri applications";
      homepage = "https://github.com/tauri-apps/tauri";
      license = with lib.licenses; [
        asl20
        mit
      ];
      mainProgram = "tauri-driver";
      platforms = lib.platforms.linux;
    };
  };

  llvm = pkgs.llvmPackages;

  executableTools = [
    rust
    node
    pnpm
    llvm.clang
    pkgs.cmake
    pkgs.ninja
    pkgs.pkg-config
    pkgs.git
    pkgs.git-lfs
    pkgs.just
    pkgs.nixfmt
    pkgs.jq
    pkgs.yq
    pkgs.python3
    pkgs.expect
    pkgs.gnutar
    pkgs.gzip
    pkgs.xz
    pkgs.zip
    pkgs.unzip
    pkgs.zstd
  ];

  commonLibraries = [
    llvm.libclang
    protobuf
    pkgs.openssl
    pkgs.sqlite
  ];

  desktopRuntime = pkgs.writeShellApplication {
    name = "eutheto-desktop-runtime";
    runtimeInputs = [ pkgs.nix ];
    text = ''
      if (( $# == 0 )); then
        printf 'usage: eutheto-desktop-runtime COMMAND [ARG ...]\n' >&2
        exit 64
      fi

      ${lib.optionalString (system == "x86_64-linux") ''
        if [[ -r /proc/driver/nvidia/version ]]; then
          exec ${pkgs.coreutils}/bin/env NIXPKGS_ALLOW_UNFREE=1 \
            ${pkgs.nix}/bin/nix \
            --extra-experimental-features 'nix-command flakes' \
            run \
            --impure \
            --no-write-lock-file \
            --override-input nixpkgs path:${nixpkgs} \
            path:${nixgl}#nixGLDefault \
            -- "$@"
        fi
      ''}

      ${lib.optionalString (system == "aarch64-linux") ''
        if [[ -r /proc/driver/nvidia/version ]]; then
          printf 'error: the repository graphics wrapper cannot provide the proprietary NVIDIA runtime on aarch64-linux\n' >&2
          exit 1
        fi
      ''}

      exec ${nixglPackages.nixGLIntel}/bin/nixGLIntel "$@"
    '';
  };

  linuxTools = [
    pkgs.patchelf
    pkgs.util-linux
    pkgs.iproute2
    pkgs.xvfb-run
    pkgs.xdotool
    pkgs.webkitgtk_4_1
    tauriDriver
    desktopRuntime
  ];
  linuxLibraries = [
    pkgs.glib
    pkgs.gtk3
    pkgs.webkitgtk_4_1
    pkgs.libsoup_3
    pkgs.librsvg
    pkgs.libayatana-appindicator
    pkgs.xdotool
  ];

  darwinTools = [ pkgs.darwin.cctools ];
  darwinLibraries = [ ];

  # nixos-25.11 marks cargo-llvm-cov-0.6.20 broken on both Darwin systems.
  qualityTools = [
    pkgs.cargo-nextest
  ]
  ++ lib.optional pkgs.stdenv.hostPlatform.isLinux pkgs.cargo-llvm-cov
  ++ [
    pkgs.cargo-insta
    pkgs.cargo-fuzz
    pkgs.cargo-deny
    pkgs.cargo-audit
  ];
in
assert lib.assertMsg (protobuf.version == "33.1") "eutheto requires exact protobuf/protoc 33.1";
assert lib.assertMsg (node.version == nodeVersion) "eutheto requires exact Node.js ${nodeVersion}";
{
  inherit
    commonLibraries
    desktopRuntime
    executableTools
    protobuf
    qualityTools
    rust
    ;
  platformTools =
    if system == "x86_64-linux" || system == "aarch64-linux" then
      linuxTools
    else if system == "x86_64-darwin" || system == "aarch64-darwin" then
      darwinTools
    else
      throw "unsupported eutheto development system: ${system}";
  platformLibraries =
    if system == "x86_64-linux" || system == "aarch64-linux" then
      linuxLibraries
    else if system == "x86_64-darwin" || system == "aarch64-darwin" then
      darwinLibraries
    else
      throw "unsupported eutheto development system: ${system}";

  # flake.nix consumes this attribute for the public full shell.
  full = qualityTools;
}
