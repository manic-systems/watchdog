{
  lib,
  rustPlatform,
  self,
}: let
  cargoToml = lib.importTOML ../Cargo.toml;
in
  rustPlatform.buildRustPackage (finalAttrs: {
    pname = "watchdog";
    version = cargoToml.package.version;
    __structuredAttrs = true;

    src = let
      fs = lib.fileset;
      s = ../.;
    in
      fs.toSource {
        root = s;
        fileset = fs.unions [
          (s + /src)
          (s + /web)
          (s + /Cargo.toml)
          (s + /Cargo.lock)
        ];
      };

    useNextest = true;
    cargoLock.lockFile = "${finalAttrs.src}/Cargo.lock";

    env = {
      WATCHDOG_COMMIT = self.rev or self.dirtyRev or "unknown";
      WATCHDOG_BUILD_DATE = self.lastModifiedDate or "unknown";
    };

    meta = {
      description = "Privacy-preserving web analytics with Prometheus-native metrics";
      homepage = "https://github.com/manic-systems/watchdog";
      license = lib.licenses.eupl12;
      maintainers = with lib.maintainers; [NotAShelf amaanq];
      mainProgram = "watchdog";
      platforms = lib.platforms.linux;
    };
  })
