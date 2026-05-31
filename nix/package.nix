{
  lib,
  rustPlatform,
}: let
  versionInfo = lib.importJSON ../version.json;
in
  rustPlatform.buildRustPackage {
    pname = "watchdog";
    version = versionInfo.version;

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

    cargoLock.lockFile = ../Cargo.lock;

    env = {
      WATCHDOG_COMMIT = versionInfo.commit;
      WATCHDOG_BUILD_DATE = versionInfo.buildDate;
    };

    meta = {
      description = "Privacy-preserving web analytics with Prometheus-native metrics";
      homepage = "https://github.com/notashelf/watchdog";
      license = lib.licenses.eupl12;
      maintainers = with lib.maintainers; [NotAShelf];
      mainProgram = "watchdog";
    };
  }
