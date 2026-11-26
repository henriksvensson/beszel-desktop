{
  description = "Live Beszel metrics in Linux desktop widgets (beszel-relay daemon, beszel-ctl CLI, and shell plugins)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        beszel-desktop = pkgs.rustPlatform.buildRustPackage {
          pname = "beszel-desktop";
          version = "0.1.0";

          src = self;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          postInstall = ''
            install -Dm644 packaging/config.example.toml $out/share/beszel-desktop/config.example.toml
            install -Dm644 LICENSE $out/share/licenses/beszel-desktop/LICENSE

            mkdir -p $out/lib/systemd/user
            sed "s|%h/.local/bin/beszel-relay|$out/bin/beszel-relay|g" \
              packaging/systemd/beszel-relay.service \
              > $out/lib/systemd/user/beszel-relay.service

            mkdir -p $out/share/beszel-desktop/shells
            cp -r shells/dms shells/noctalia shells/waybar $out/share/beszel-desktop/shells/
          '';

          meta = with pkgs.lib; {
            description = "Live Beszel metrics daemon, CLI, and Wayland desktop shell widgets";
            homepage = "https://github.com/henriksvensson/beszel-desktop";
            license = licenses.mit;
            platforms = platforms.linux;
            mainProgram = "beszel-ctl";
          };
        };

        default = beszel-desktop;
      });
    };
}
