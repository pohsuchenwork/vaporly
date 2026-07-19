# Home-manager module for Vaporly speech-to-text
#
# Provides a systemd user service for autostart.
# Usage: imports = [ vaporly.homeManagerModules.default ];
#        services.vaporly.enable = true;
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.vaporly;
in
{
  options.services.vaporly = {
    enable = lib.mkEnableOption "Vaporly speech-to-text user service";

    package = lib.mkOption {
      type = lib.types.package;
      defaultText = lib.literalExpression "vaporly.packages.\${system}.vaporly";
      description = "The Vaporly package to use.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.vaporly = {
      Unit = {
        Description = "Vaporly speech-to-text";
        After = [ "graphical-session.target" ];
        PartOf = [ "graphical-session.target" ];
      };
      Service = {
        ExecStart = "${cfg.package}/bin/vaporly";
        Restart = "on-failure";
        RestartSec = 5;
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
