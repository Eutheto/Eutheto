{
  pkgs,
  src,
  solver-artifact-tool,
}:
let
  inherit (pkgs) lib stdenv;

  contractPath = ../workers/ortools/source-contract.json;
  repositoryPatchPath = ../workers/ortools/patches/9.15-candidate-fixes.patch;
  protocolSchemaPath = ../protocol/solver-worker.proto;
  contract = builtins.fromJSON (builtins.readFile contractPath);
  dependencySourcesPath = ../workers/ortools/dependency-sources.json;
  dependencyLock = builtins.fromJSON (builtins.readFile dependencySourcesPath);
  dependencySources = dependencyLock.dependencies;
  expectedDependencyNames = [
    "abseil"
    "bzip2"
    "eigen"
    "re2"
    "zlib"
  ];
  isHttpsUrl = value: builtins.isString value && builtins.match "https://.+" value != null;
  isLowerSha256 =
    value:
    builtins.isString value
    && builtins.stringLength value == 64
    && builtins.match "[0-9a-f]+" value != null;
  isSafeLeaf =
    value:
    builtins.isString value
    && value != "."
    && value != ".."
    && builtins.match "[A-Za-z0-9][A-Za-z0-9._+-]*" value != null;
  dependencyValuesAreSafe = builtins.all (
    name:
    let
      dependency = dependencySources.${name};
    in
    isHttpsUrl dependency.source_url
    && isLowerSha256 dependency.sha256
    && isSafeLeaf dependency.archive_name
    && isSafeLeaf dependency.archive_root
    && isSafeLeaf dependency.patch
  ) expectedDependencyNames;
  dependencyFieldsAreExact = builtins.all (
    name:
    builtins.attrNames dependencySources.${name} == [
      "archive_name"
      "archive_root"
      "patch"
      "sha256"
      "source_url"
      "version"
    ]
  ) expectedDependencyNames;

  supportedSystems = [
    "x86_64-linux"
    "x86_64-darwin"
    "aarch64-darwin"
  ];
  linuxRuntimeLibraryPath = lib.makeLibraryPath [ stdenv.cc.cc.lib ];
  targetTriple =
    {
      "x86_64-linux" = "x86_64-unknown-linux-gnu";
      "x86_64-darwin" = "x86_64-apple-darwin";
      "aarch64-darwin" = "aarch64-apple-darwin";
    }
    .${stdenv.hostPlatform.system};
  compilerIdentity = if stdenv.cc.isClang then "clang" else "gcc";
  compilerVersion = lib.getVersion stdenv.cc.cc;
  solverSourceDate = "2026-09-02T00:00:00Z";
  ortoolsInstallLibDir = if stdenv.hostPlatform.isLinux then "lib64" else "lib";

  cmakeValue = value: if builtins.isBool value then if value then "ON" else "OFF" else toString value;
  commonCmakeFlags = lib.mapAttrsToList (
    name: value: "-D${name}=${cmakeValue value}"
  ) contract.cmake.cache_entries;
  commonCmakeFlagsShell = lib.escapeShellArgs commonCmakeFlags;
  outputPath = builtins.placeholder "out";
  darwinCmakeFlagsShell = lib.escapeShellArgs (
    lib.optionals stdenv.hostPlatform.isDarwin [
      "-DCMAKE_INSTALL_NAME_DIR=@rpath"
      "-DCMAKE_INSTALL_RPATH=${outputPath}/lib"
      "-DCMAKE_MACOSX_RPATH=ON"
    ]
  );

  fetchArchive =
    {
      name,
      url,
      sha256,
    }:
    pkgs.fetchurl {
      inherit name url sha256;
    };

  ortoolsSource = fetchArchive {
    name = "or-tools-v9.15.tar.gz";
    url = contract.ortools.source_url;
    sha256 = contract.ortools.sha256;
  };
  protobufSource = fetchArchive {
    name = "protobuf-33.1.tar.gz";
    url = contract.protobuf.source_url;
    sha256 = contract.protobuf.sha256;
  };
  transitiveSources = lib.mapAttrs (
    _: dependency:
    fetchArchive {
      name = dependency.archive_name;
      url = dependency.source_url;
      sha256 = dependency.sha256;
    }
  ) dependencySources;
in
assert contract.schema_version == 1;
assert contract.approval.phase == 3;
assert contract.approval.status == "approved";
assert contract.ortools.version == "9.15.6755";
assert contract.ortools.patch_path == "workers/ortools/patches/9.15-candidate-fixes.patch";
assert builtins.hashFile "sha256" repositoryPatchPath == contract.ortools.patch_sha256;
assert contract.protobuf.source_version == "33.1";
assert contract.protobuf.protoc_version == "33.1";
assert contract.protobuf.cpp_runtime_version == "33.1.0";
assert contract.protocol.wire_version == 1;
assert builtins.hashFile "sha256" protocolSchemaPath == contract.protocol.schema_sha256;
assert contract.worker.identity == "eutheto-ortools-worker";
assert contract.worker.version == "0.1.0";
assert builtins.isAttrs dependencyLock;
assert
  builtins.attrNames dependencyLock == [
    "dependencies"
    "ortools"
    "schema_version"
  ];
assert dependencyLock.schema_version == 1;
assert builtins.isAttrs dependencyLock.ortools;
assert
  builtins.attrNames dependencyLock.ortools == [
    "sha256"
    "version"
  ];
assert dependencyLock.ortools.version == contract.ortools.version;
assert dependencyLock.ortools.sha256 == contract.ortools.sha256;
assert isLowerSha256 dependencyLock.ortools.sha256;
assert builtins.isAttrs dependencySources;
assert builtins.attrNames dependencySources == expectedDependencyNames;
assert dependencyFieldsAreExact;
assert dependencySources.abseil.version == "20250814.1";
assert dependencySources.bzip2.version == "66c46b8c9436613fd81bc5d03f63a61933a4dcc3";
assert dependencySources.eigen.version == "3.4.0";
assert dependencySources.re2.version == "2025-08-12";
assert dependencySources.zlib.version == "1.4";
assert dependencyValuesAreSafe;
assert builtins.elem stdenv.hostPlatform.system supportedSystems;
stdenv.mkDerivation {
  pname = contract.worker.identity;
  version = contract.worker.version;

  strictDeps = true;
  OR_TOOLS_PATCH = "6755";

  nativeBuildInputs = [
    pkgs.cmake
    pkgs.git
    pkgs.gnutar
    pkgs.gzip
    pkgs.ninja
    pkgs.pkg-config
    pkgs.python3
  ]
  ++ lib.optionals stdenv.hostPlatform.isLinux [ pkgs.patchelf ]
  ++ lib.optionals stdenv.hostPlatform.isDarwin [ pkgs.darwin.cctools ];
  buildInputs = lib.optionals stdenv.hostPlatform.isLinux [ stdenv.cc.cc.lib ];

  unpackPhase = ''
    runHook preUnpack

    mkdir -p "$NIX_BUILD_TOP/sources"
    tar -xzf ${ortoolsSource} -C "$NIX_BUILD_TOP/sources"
    tar -xzf ${protobufSource} -C "$NIX_BUILD_TOP/sources"
    tar -xzf ${transitiveSources.zlib} -C "$NIX_BUILD_TOP/sources"
    tar -xzf ${transitiveSources.bzip2} -C "$NIX_BUILD_TOP/sources"
    tar -xzf ${transitiveSources.abseil} -C "$NIX_BUILD_TOP/sources"
    tar -xzf ${transitiveSources.re2} -C "$NIX_BUILD_TOP/sources"
    tar -xzf ${transitiveSources.eigen} -C "$NIX_BUILD_TOP/sources"

    sourceRoot="$NIX_BUILD_TOP/sources/or-tools-9.15"
    test -d "$sourceRoot"
    test -d "$NIX_BUILD_TOP/sources/protobuf-33.1"
    test -d "$NIX_BUILD_TOP/sources/${dependencySources.zlib.archive_root}"
    test -d "$NIX_BUILD_TOP/sources/${dependencySources.bzip2.archive_root}"
    test -d "$NIX_BUILD_TOP/sources/${dependencySources.abseil.archive_root}"
    test -d "$NIX_BUILD_TOP/sources/${dependencySources.re2.archive_root}"
    test -d "$NIX_BUILD_TOP/sources/${dependencySources.eigen.archive_root}"
    cd "$sourceRoot"

    runHook postUnpack
  '';

  patchPhase = ''
    runHook prePatch

    git apply --check ${src}/workers/ortools/patches/9.15-candidate-fixes.patch
    git apply ${src}/workers/ortools/patches/9.15-candidate-fixes.patch

    (
      cd "$NIX_BUILD_TOP/sources/${dependencySources.zlib.archive_root}"
      git apply --check --ignore-whitespace "$sourceRoot/patches/${dependencySources.zlib.patch}"
      git apply --ignore-whitespace "$sourceRoot/patches/${dependencySources.zlib.patch}"
    )
    (
      cd "$NIX_BUILD_TOP/sources/${dependencySources.bzip2.archive_root}"
      git apply --check --ignore-whitespace "$sourceRoot/patches/${dependencySources.bzip2.patch}"
      git apply --ignore-whitespace "$sourceRoot/patches/${dependencySources.bzip2.patch}"
    )
    (
      cd "$NIX_BUILD_TOP/sources/${dependencySources.abseil.archive_root}"
      git apply --check --ignore-whitespace "$sourceRoot/patches/${dependencySources.abseil.patch}"
      git apply --ignore-whitespace "$sourceRoot/patches/${dependencySources.abseil.patch}"
    )
    (
      cd "$NIX_BUILD_TOP/sources/${dependencySources.re2.archive_root}"
      git apply --check --ignore-whitespace "$sourceRoot/patches/${dependencySources.re2.patch}"
      git apply --ignore-whitespace "$sourceRoot/patches/${dependencySources.re2.patch}"
    )
    (
      cd "$NIX_BUILD_TOP/sources/${dependencySources.eigen.archive_root}"
      git apply --check --ignore-whitespace "$sourceRoot/patches/${dependencySources.eigen.patch}"
      git apply --ignore-whitespace "$sourceRoot/patches/${dependencySources.eigen.patch}"
    )

    runHook postPatch
  '';

  configurePhase = ''
    runHook preConfigure

    cmake \
      -S "$sourceRoot" \
      -B "$NIX_BUILD_TOP/ortools-build" \
      -G Ninja \
      ${commonCmakeFlagsShell} \
      ${darwinCmakeFlagsShell} \
      "-DCMAKE_CXX_FLAGS=-DEIGEN_MPL2_ONLY" \
      "-DCMAKE_INSTALL_PREFIX=$NIX_BUILD_TOP/ortools-install" \
      "-DCMAKE_POLICY_DEFAULT_CMP0077=NEW" \
      "-DFETCHCONTENT_FULLY_DISCONNECTED=ON" \
      "-DFETCHCONTENT_SOURCE_DIR_ZLIB=$NIX_BUILD_TOP/sources/${dependencySources.zlib.archive_root}" \
      "-DFETCHCONTENT_SOURCE_DIR_BZIP2=$NIX_BUILD_TOP/sources/${dependencySources.bzip2.archive_root}" \
      "-DFETCHCONTENT_SOURCE_DIR_ABSL=$NIX_BUILD_TOP/sources/${dependencySources.abseil.archive_root}" \
      "-DFETCHCONTENT_SOURCE_DIR_PROTOBUF=$NIX_BUILD_TOP/sources/protobuf-33.1" \
      "-DFETCHCONTENT_SOURCE_DIR_RE2=$NIX_BUILD_TOP/sources/${dependencySources.re2.archive_root}" \
      "-DFETCHCONTENT_SOURCE_DIR_EIGEN3=$NIX_BUILD_TOP/sources/${dependencySources.eigen.archive_root}"

    runHook postConfigure
  '';

  buildPhase = ''
    runHook preBuild

    cmake --build "$NIX_BUILD_TOP/ortools-build" --parallel "$NIX_BUILD_CORES"
    cmake --install "$NIX_BUILD_TOP/ortools-build"

    cmake \
      -S ${src}/workers/ortools \
      -B "$NIX_BUILD_TOP/worker-build" \
      -G Ninja \
      ${commonCmakeFlagsShell} \
      ${darwinCmakeFlagsShell} \
      "-DCMAKE_INSTALL_PREFIX=$out" \
      "-DCMAKE_PREFIX_PATH=$NIX_BUILD_TOP/ortools-install" \
      "-Dortools_DIR=$NIX_BUILD_TOP/ortools-install/${ortoolsInstallLibDir}/cmake/ortools" \
      "-DProtobuf_DIR=$NIX_BUILD_TOP/ortools-install/${ortoolsInstallLibDir}/cmake/protobuf" \
      "-DCMAKE_FIND_USE_CMAKE_ENVIRONMENT_PATH=FALSE" \
      "-DCMAKE_FIND_USE_PACKAGE_ROOT_PATH=FALSE" \
      "-DCMAKE_FIND_USE_PACKAGE_REGISTRY=FALSE" \
      "-DCMAKE_FIND_USE_SYSTEM_PACKAGE_REGISTRY=FALSE" \
      "-DEUTHETO_ORTOOLS_DEVELOPMENT_BUILD=OFF" \
      "-DEUTHETO_ORTOOLS_BUILD_TESTS=ON" \
      "-DEUTHETO_ORTOOLS_BUILD_CANDIDATE_BENCHMARKS=OFF" \
      "-DEUTHETO_ORTOOLS_PHASE3_CONTRACT=${src}/workers/ortools/source-contract.json"
    cmake --build "$NIX_BUILD_TOP/worker-build" --parallel "$NIX_BUILD_CORES"

    runHook postBuild
  '';

  doCheck = stdenv.buildPlatform.canExecute stdenv.hostPlatform;
  checkPhase = ''
    runHook preCheck

    ctest \
      --test-dir "$NIX_BUILD_TOP/worker-build" \
      --output-on-failure \
      --no-tests=error

    runHook postCheck
  '';

  installPhase = ''
    runHook preInstall

    cmake --install "$NIX_BUILD_TOP/worker-build" --prefix "$out"
    test -x "$out/bin/ortools-worker"

    runtime_dependencies() {
      if ${if stdenv.hostPlatform.isLinux then "true" else "false"}; then
        patchelf --print-needed "$1"
      else
        otool -L "$1" | awk 'NR > 1 { print $1 }'
      fi
    }

    is_forbidden_dependency() {
      case "$1" in
        *GLPK*|*glpk*|*gurobi*|*Gurobi*|*cplex*|*CPLEX*|*xpress*|*XPRESS*|\
        *python*|*Python*|*libjvm*|*java*|*Java*|*coreclr*|*hostfxr*|\
        *dotnet*|*DotNet*)
          return 0
          ;;
      esac
      return 1
    }

    is_system_dependency() {
      case "$1" in
        libc.so.*|libm.so.*|libdl.so.*|libpthread.so.*|librt.so.*|\
        libresolv.so.*|libutil.so.*|libstdc++.so.*|libgcc_s.so.*|\
        libatomic.so.*|libgomp.so.*|libquadmath.so.*|ld-linux*.so.*|\
        linux-vdso.so.*|/usr/lib/*|/System/Library/*)
          return 0
          ;;
      esac
      return 1
    }

    runtimeQueue="$NIX_BUILD_TOP/runtime-library-queue"
    runtimeSeen="$NIX_BUILD_TOP/runtime-library-seen"
    runtime_dependencies "$out/bin/ortools-worker" >"$runtimeQueue"
    : >"$runtimeSeen"
    mkdir -p "$out/lib"
    runtimeLibraryCount=0

    while IFS= read -r dependency; do
      test -n "$dependency" || continue
      if is_forbidden_dependency "$dependency"; then
        echo "forbidden worker runtime dependency detected: $dependency" >&2
        exit 1
      fi
      is_system_dependency "$dependency" && continue

      runtimeName="$(basename "$dependency")"
      sourceLibrary=
      for libraryDirectory in \
        "$NIX_BUILD_TOP/ortools-install/lib" \
        "$NIX_BUILD_TOP/ortools-install/lib64"
      do
        if test -e "$libraryDirectory/$runtimeName"; then
          if test -n "$sourceLibrary"; then
            echo "runtime loader basename collides across staged directories: $runtimeName" >&2
            exit 1
          fi
          sourceLibrary="$libraryDirectory/$runtimeName"
        fi
      done
      if test -z "$sourceLibrary"; then
        echo "worker runtime dependency is not in the staged closure: $dependency" >&2
        exit 1
      fi

      linkDepth=0
      currentLibrary="$sourceLibrary"
      while test -L "$currentLibrary"; do
        linkDepth=$((linkDepth + 1))
        if test "$linkDepth" -gt 32; then
          echo "runtime library symlink chain is unbounded: $sourceLibrary" >&2
          exit 1
        fi
        linkTarget="$(readlink "$currentLibrary")"
        case "$linkTarget" in
          /*|*/*)
            echo "runtime library has a non-local symlink target: $currentLibrary -> $linkTarget" >&2
            exit 1
            ;;
        esac
        currentLibrary="$(dirname "$currentLibrary")/$linkTarget"
        if ! test -e "$currentLibrary"; then
          echo "runtime library symlink is unresolved: $sourceLibrary" >&2
          exit 1
        fi
      done
      test -f "$currentLibrary" && ! test -L "$currentLibrary"

      if grep -Fqx "$runtimeName" "$runtimeSeen"; then
        continue
      fi
      printf '%s\n' "$runtimeName" >>"$runtimeSeen"
      runtimeDestination="$out/lib/$runtimeName"
      if test -e "$runtimeDestination" || test -L "$runtimeDestination"; then
        echo "runtime output basename collision: $runtimeName" >&2
        exit 1
      fi
      cp --dereference "$currentLibrary" "$runtimeDestination"
      test -f "$runtimeDestination" && ! test -L "$runtimeDestination"
      runtime_dependencies "$runtimeDestination" >>"$runtimeQueue"
      runtimeLibraryCount=$((runtimeLibraryCount + 1))
    done <"$runtimeQueue"

    if test "$runtimeLibraryCount" -eq 0; then
      rmdir "$out/lib"
    fi

    runHook postInstall
  '';

  postFixup = ''
    ${lib.optionalString stdenv.hostPlatform.isLinux ''
      patchelf --set-rpath '$ORIGIN/../lib' "$out/bin/ortools-worker"
      patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 "$out/bin/ortools-worker"
      for binary in "$out"/lib/*.so "$out"/lib/*.so.*; do
        test -e "$binary" || continue
        test -L "$binary" && exit 1
        patchelf --set-rpath '$ORIGIN' "$binary"
      done
    ''}
    ${lib.optionalString stdenv.hostPlatform.isDarwin ''
      rewrite_rpaths() {
        artifact="$1"
        relativeRpath="$2"
        while IFS= read -r rpath; do
          test -n "$rpath" || continue
          install_name_tool -delete_rpath "$rpath" "$artifact"
        done < <(otool -l "$artifact" | awk '
          $1 == "cmd" && $2 == "LC_RPATH" { in_rpath = 1; next }
          in_rpath && $1 == "path" { print $2; in_rpath = 0 }
        ')
        install_name_tool -add_rpath "$relativeRpath" "$artifact"
      }

      rewrite_rpaths "$out/bin/ortools-worker" '@loader_path/../lib'
      for binary in "$out"/lib/*.dylib; do
        test -e "$binary" || continue
        test -L "$binary" && exit 1
        install_name_tool -id "@rpath/$(basename "$binary")" "$binary"
        rewrite_rpaths "$binary" '@loader_path'
      done
    ''}
    if find "$out" -type l -print -quit | grep -q .; then
      echo "solver artifact contains a forbidden symlink before finalization" >&2
      exit 1
    fi
    ${solver-artifact-tool}/bin/xtask solver finalize-artifact \
      --authority-root ${src} \
      --work-root "$NIX_BUILD_TOP" \
      --artifact-root "$out" \
      --target-triple ${targetTriple} \
      --compiler-identity ${compilerIdentity} \
      --compiler-version ${compilerVersion} \
      --source-date ${solverSourceDate}
    rm -rf "$NIX_BUILD_TOP/ortools-install"
  '';

  doInstallCheck = stdenv.buildPlatform.canExecute stdenv.hostPlatform;
  installCheckPhase = ''
    runHook preInstallCheck

    workerUnderTest="$out/bin/ortools-worker"
    ${lib.optionalString stdenv.hostPlatform.isLinux ''
      workerUnderTest="$NIX_BUILD_TOP/ortools-worker-install-check"
      cp "$out/bin/ortools-worker" "$workerUnderTest"
      patchelf \
        --set-interpreter ${stdenv.cc.bintools.dynamicLinker} \
        --set-rpath "$out/lib:${linuxRuntimeLibraryPath}" \
        "$workerUnderTest"
    ''}
    set +e
    "$workerUnderTest" </dev/null >worker.stdout 2>worker.stderr
    workerStatus=$?
    set -e
    if test "$workerStatus" -ne 64; then
      echo "installed worker returned $workerStatus for empty stdin; expected 64" >&2
      cat worker.stderr >&2
      exit 1
    fi
    if test -s worker.stdout; then
      echo "installed worker wrote protocol output for empty stdin" >&2
      cat worker.stdout >&2
      exit 1
    fi

    runtime_dependencies() {
      if ${if stdenv.hostPlatform.isLinux then "true" else "false"}; then
        patchelf --print-needed "$1"
      else
        otool -L "$1" | awk 'NR > 1 { print $1 }'
      fi
    }

    runtime_paths() {
      if ${if stdenv.hostPlatform.isLinux then "true" else "false"}; then
        patchelf --print-rpath "$1"
      else
        otool -l "$1" | awk '
          $1 == "cmd" && $2 == "LC_RPATH" { in_rpath = 1; next }
          in_rpath && $1 == "path" { print $2; in_rpath = 0 }
        '
      fi
    }

    is_forbidden_dependency() {
      case "$1" in
        *GLPK*|*glpk*|*gurobi*|*Gurobi*|*cplex*|*CPLEX*|*xpress*|*XPRESS*|\
        *python*|*Python*|*libjvm*|*java*|*Java*|*coreclr*|*hostfxr*|\
        *dotnet*|*DotNet*)
          return 0
          ;;
      esac
      return 1
    }

    is_system_dependency() {
      case "$1" in
        libc.so.*|libm.so.*|libdl.so.*|libpthread.so.*|librt.so.*|\
        libresolv.so.*|libutil.so.*|libstdc++.so.*|libgcc_s.so.*|\
        libatomic.so.*|libgomp.so.*|libquadmath.so.*|ld-linux*.so.*|\
        linux-vdso.so.*|/usr/lib/*|/System/Library/*)
          return 0
          ;;
      esac
      return 1
    }

    for artifact in "$out/bin/ortools-worker" "$out"/lib/*; do
      test -e "$artifact" || continue
      if test -L "$artifact" || ! test -f "$artifact"; then
        echo "installed solver payload entry is not a regular file: $artifact" >&2
        exit 1
      fi

      linkage="$(runtime_dependencies "$artifact")"
      runtimePaths="$(runtime_paths "$artifact")"
      if is_forbidden_dependency "$linkage"; then
        echo "forbidden runtime dependency detected in $artifact" >&2
        printf '%s\n' "$linkage" >&2
        exit 1
      fi
      case "$runtimePaths" in
        *"$NIX_BUILD_TOP"*|*/build/*)
          echo "runtime loader metadata references the private build tree: $artifact" >&2
          printf '%s\n' "$runtimePaths" >&2
          exit 1
          ;;
      esac
      case "$linkage:$runtimePaths" in
        *"/nix/store/"*)
          echo "installed solver payload retains a Nix-store loader reference: $artifact" >&2
          printf '%s\n%s\n' "$linkage" "$runtimePaths" >&2
          exit 1
          ;;
      esac

      while IFS= read -r dependency; do
        test -n "$dependency" || continue
        is_system_dependency "$dependency" && continue
        runtimeName="$(basename "$dependency")"
        if ! test -e "$out/lib/$runtimeName"; then
          echo "installed runtime dependency is unresolved: $artifact -> $dependency" >&2
          exit 1
        fi
      done <<<"$linkage"
    done

    workerRuntimePaths="$(runtime_paths "$out/bin/ortools-worker")"
    ${lib.optionalString stdenv.hostPlatform.isLinux ''
      if test "$workerRuntimePaths" != '$ORIGIN/../lib'; then
        echo "installed worker does not use only the packaged relative runtime directory" >&2
        printf '%s\n' "$workerRuntimePaths" >&2
        exit 1
      fi
    ''}
    ${lib.optionalString stdenv.hostPlatform.isDarwin ''
      if test "$workerRuntimePaths" != '@loader_path/../lib'; then
        echo "installed worker does not use only the packaged relative runtime directory" >&2
        printf '%s\n' "$workerRuntimePaths" >&2
        exit 1
      fi
      for binary in "$out"/lib/*.dylib; do
        test -e "$binary" || continue
        libraryRuntimePaths="$(runtime_paths "$binary")"
        if test "$libraryRuntimePaths" != '@loader_path'; then
          echo "installed runtime library does not use only its packaged directory" >&2
          printf '%s\n' "$libraryRuntimePaths" >&2
          exit 1
        fi
      done
    ''}

    ${lib.optionalString stdenv.hostPlatform.isLinux ''
      workerInterpreter="$(patchelf --print-interpreter "$out/bin/ortools-worker")"
      if test "$workerInterpreter" != /lib64/ld-linux-x86-64.so.2; then
        echo "installed worker uses a non-portable ELF interpreter: $workerInterpreter" >&2
        exit 1
      fi
      case "$workerRuntimePaths" in
        *"/nix/store/"*)
          echo "installed worker retains a Nix-store runtime path" >&2
          printf '%s\n' "$workerRuntimePaths" >&2
          exit 1
          ;;
      esac
    ''}

    if find "$out" -type l -print -quit | grep -q .; then
      echo "installed solver artifact contains a symlink" >&2
      exit 1
    fi
    ${solver-artifact-tool}/bin/xtask solver validate-manifest \
      --source-contract ${src}/workers/ortools/source-contract.json \
      --protocol-schema ${src}/protocol/solver-worker.proto \
      --protocol-policy ${src}/protocol/version.json \
      --artifact-root "$out"
    echo "installed finalized worker, runtime closure, and manifest checks passed"

    runHook postInstallCheck
  '';

  passthru = {
    inherit contract;
    sourceContractApproved = true;
    nixWorkerArtifactBuildable = true;
    solverManifestAvailable = true;
    sbomAvailable = true;
    licensePayloadAvailable = true;
    packagingAvailable = false;
    backendAvailable = false;
    releaseReady = false;
  };

  meta = {
    description = "Pinned OR-Tools CP-SAT worker for eutheto";
    homepage = "https://github.com/google/or-tools";
    license = with lib.licenses; [
      asl20
      bsd3
      bsdOriginal
      mit
      zlib
    ];
    mainProgram = "ortools-worker";
    platforms = supportedSystems;
  };
}
