{ lib }:
root:
let
  rootString = toString root;
  excludedDirectories = [
    ".cache"
    ".direnv"
    ".git"
    ".hg"
    ".idea"
    ".next"
    ".npm"
    ".nuxt"
    ".nyc_output"
    ".output"
    ".pnpm-store"
    ".svn"
    ".tmp"
    ".turbo"
    ".vite"
    ".vscode"
    ".yarn"
    "CMakeFiles"
    "allure-results"
    "build"
    "coverage"
    "dist"
    "node_modules"
    "playwright-report"
    "pdf-sidecar"
    "pdf-sidecar.staging"
    "pdf-sidecar.previous"
    "result"
    "target"
    "test-results"
    "tmp"
  ];
  excludedFiles = [
    ".AppleDouble"
    ".DS_Store"
    ".LSOverride"
    "CMakeCache.txt"
    "Desktop.ini"
    "Thumbs.db"
    "compile_commands.json"
    "npm-debug.log"
    "pnpm-debug.log"
    "pdf-sidecar.lock"
    "yarn-error.log"
  ];
  excludedSuffixes = [
    ".AppImage"
    ".db"
    ".db-shm"
    ".db-wal"
    ".deb"
    ".dmg"
    ".exe"
    ".jks"
    ".key"
    ".log"
    ".msi"
    ".msix"
    ".p12"
    ".pdb"
    ".pem"
    ".pfx"
    ".pkg"
    ".profraw"
    ".rpm"
    ".rs.bk"
    ".sqlite"
    ".sqlite3"
    ".swo"
    ".swp"
    ".tmp"
    ".tsbuildinfo"
    "~"
  ];
  isSecretEnvironmentFile =
    name: (name == ".env" || lib.hasPrefix ".env." name) && name != ".env.example";
  filter =
    path: _type:
    let
      pathString = toString path;
      relative = lib.removePrefix "${rootString}/" pathString;
      components = lib.splitString "/" relative;
      name = builtins.baseNameOf pathString;
      hasExcludedDirectory = lib.any (
        component:
        builtins.elem component excludedDirectories
        || lib.hasPrefix "cmake-build-" component
        || lib.hasPrefix "result-" component
      ) components;
      hasExcludedSuffix = lib.any (suffix: lib.hasSuffix suffix name) excludedSuffixes;
    in
    !hasExcludedDirectory
    && !(builtins.elem name excludedFiles)
    && !isSecretEnvironmentFile name
    && !hasExcludedSuffix;
in
lib.cleanSourceWith {
  name = "eutheto-source";
  src = root;
  inherit filter;
}
