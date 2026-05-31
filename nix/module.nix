self: {
  config,
  pkgs,
  lib,
  ...
}: let
  inherit (lib.modules) mkIf;
  inherit (lib.options) mkOption mkEnableOption mkPackageOption;
  inherit (lib.types) str path bool;

  settingsFormat = pkgs.formats.toml {};
  settingsType = settingsFormat.type;

  cfg = config.services.watchdog;
in {
  meta.maintainers = with lib.maintainers; [NotAShelf];

  options.services.watchdog = {
    enable = mkEnableOption "Watchdog privacy-preserving analytics";

    package = mkPackageOption self.packages.${pkgs.stdenv.hostPlatform.system} "watchdog" {
      pkgsText = "self.packages.\${pkgs.stdenv.hostPlatform.system}";
    };

    settings = mkOption {
      type = settingsType;
      default = {};
      description = ''
        TOML configuration for Watchdog analytics.

        See <https://github.com/notashelf/watchdog> for available options.
      '';
      example = {
        site = {
          domains = ["example.com"];
          salt_rotation = "daily";
          sampling = 1.0;
          collect = {
            pageviews = true;
            country = false;
            device = true;
            referrer = "domain";
          };

          path = {
            strip_query = true;
            strip_fragment = true;
            collapse_numeric_segments = true;
            max_segments = 5;
            normalize_trailing_slash = true;
          };
        };

        limits = {
          max_paths = 10000;
          max_sources = 500;
          max_custom_events = 100;
          max_events_per_minute = 10000;
          device_breakpoints = {
            mobile = 768;
            tablet = 1024;
          };
        };

        security = {
          trusted_proxies = ["127.0.0.1" "::1"];
          cors = {
            enabled = false;
            allowed_origins = ["*"];
          };
          metrics_auth = {
            enabled = false;
          };
        };

        server = {
          listen_addr = "127.0.0.1:8080";
          metrics_path = "/metrics";
          ingestion_path = "/api/event";
        };
      };
    };

    user = mkOption {
      type = str;
      default = "watchdog";
      description = "User account under which Watchdog runs.";
    };

    group = mkOption {
      type = str;
      default = "watchdog";
      description = "Group under which Watchdog runs.";
    };

    stateDir = mkOption {
      type = path;
      default = "/var/lib/watchdog";
      description = "Directory for Watchdog state (HLL persistence).";
    };

    openFirewall = mkOption {
      type = bool;
      default = false;
      description = "Open firewall port for Watchdog.";
    };
  };

  config = mkIf cfg.enable {
    systemd.services.watchdog = let
      settings =
        lib.recursiveUpdate {
          server.state_path = "${cfg.stateDir}/hll.state";
        }
        cfg.settings;
      configFile = settingsFormat.generate "watchdog.toml" settings;
    in {
      description = "Watchdog Privacy Analytics";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;

        ExecStart = "${lib.getExe cfg.package} --config ${configFile}";

        Restart = "on-failure";
        RestartSec = "5s";

        # Security hardening
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ReadWritePaths = [cfg.stateDir];

        # Capabilities
        AmbientCapabilities = "";
        CapabilityBoundingSet = "";

        # Sandboxing
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictAddressFamilies = ["AF_INET" "AF_INET6" "AF_UNIX"];
        RestrictNamespaces = true;
        LockPersonality = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        RemoveIPC = true;
        PrivateMounts = true;

        # System call filtering
        SystemCallFilter = ["@system-service" "~@privileged" "~@resources"];
        SystemCallErrorNumber = "EPERM";
        SystemCallArchitectures = "native";
      };
    };

    users = {
      users = mkIf (cfg.user == "watchdog") {
        watchdog = {
          description = "Watchdog analytics daemon user";
          isSystemUser = true;
          group = cfg.group;
          home = cfg.stateDir;
        };
      };

      groups = mkIf (cfg.group == "watchdog") {
        watchdog = {};
      };
    };

    systemd.tmpfiles.rules = [
      "d '${cfg.stateDir}' 0750 ${cfg.user} ${cfg.group} - -"
    ];

    networking.firewall = mkIf cfg.openFirewall {
      allowedTCPPorts = let
        # Extract port from listen_addr (format: "host:port" or ":port")
        listenAddr = cfg.settings.server.listen_addr or "127.0.0.1:8080";
        port = lib.toInt (lib.last (lib.splitString ":" listenAddr));
      in [port];
    };
  };
}
