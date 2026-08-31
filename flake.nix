{
  description = "janis — devShell + app package, composed from chess-flake bundles";

  # The workspace owns the toolchain pins (Rust channel, Node version, pnpm,
  # prettier) and the shared shell shapes. Janis is not a chess project, but
  # the tauriShell bundle is domain-agnostic: Rust + Node + macOS pkg-config.
  inputs.workspace.url = "github:obazin/chess-flake";

  outputs =
    { self, workspace }:
    let
      perSystem = builtins.mapAttrs (
        system: lib:
        let
          pkgs = lib.pkgs;

          # `run` — launch the app in dev (`just run` → `pnpm tauri dev`).
          # A wrapper on PATH rather than a shell alias so it works in zsh,
          # bash, `nix develop` and non-interactive shells alike.
          shorthands = [
            (pkgs.writeShellScriptBin "run" ''
              exec just run "$@"
            '')
          ];

          # The audio engine needs more than the bundle's Rust + Node.
          #
          # cmake: the `opus` crate builds libopus from bundled source via
          # `opusic-sys`, which drives cmake. Without it the Opus decoder —
          # and so the whole backend — does not compile.
          audioTools = [ pkgs.cmake ];

          # alsa-lib: cpal's Linux backend links ALSA through pkg-config.
          # macOS reaches CoreAudio through the SDK and needs nothing here.
          audioLibs = pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.alsa-lib ];

          # glib-networking and gsettings-desktop-schemas are runtime pieces
          # webkit needs (TLS gio module, GSettings schemas), not link libs.
          tauriLinuxLibs =
            with pkgs;
            pkgs.lib.optionals pkgs.stdenv.isLinux [
              dbus
              glib
              gtk3
              libsoup_3
              webkitgtk_4_1
              glib-networking
              gsettings-desktop-schemas
            ];

          # Shell-built binaries get no wrapper, so the GTK runtime lookup
          # dirs must be exported here. Prepended: the bundle's own hook
          # exec's into zsh, so anything appended after it would never run.
          tauriLinuxShellEnv = pkgs.lib.optionalString pkgs.stdenv.isLinux ''
            export XDG_DATA_DIRS="${pkgs.lib.makeSearchPath "share/gsettings-schemas" [
              pkgs.gsettings-desktop-schemas
              pkgs.gtk3
            ]}''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
            export GIO_EXTRA_MODULES="${pkgs.glib-networking}/lib/gio/modules''${GIO_EXTRA_MODULES:+:$GIO_EXTRA_MODULES}"
          '';

          # Keep `version` in sync with Cargo.toml and tauri.conf.json.
          janis = pkgs.rustPlatform.buildRustPackage (finalAttrs: {
            pname = "janis";
            version = "0.1.0";

            src = ./.;
            cargoRoot = "src-tauri";
            buildAndTestSubdir = finalAttrs.cargoRoot;

            cargoHash = "sha256-l+kYgFzGrsL6urrHg91uCwdZ/y7sBRzFNPB9BqXuzlQ=";

            pnpmDeps = pkgs.fetchPnpmDeps {
              inherit (finalAttrs)
                pname
                version
                src
                ;
              pnpm = pkgs.pnpm_10;
              fetcherVersion = 3;
              hash = "sha256-Wooi727ud7ICPZFC4iU9YLc0f3aOjetxCB3o8ndMTi0=";
            };

            nativeBuildInputs = [
              pkgs.cmake
              pkgs.cargo-tauri.hook
              pkgs.nodejs_22
              pkgs.pkg-config
              pkgs.pnpm_10
              pkgs.pnpmConfigHook
              pkgs.wrapGAppsHook3
            ];

            buildInputs = tauriLinuxLibs ++ audioLibs;

            meta = {
              description = "Open-source desktop audio player: local library, web radio, 10-band EQ and live visualisation.";
              homepage = "https://github.com/obazin/janis";
              license = pkgs.lib.licenses.gpl3Only;
              mainProgram = "Janis";
              platforms = pkgs.lib.platforms.linux ++ pkgs.lib.platforms.darwin;
            };
          });
        in
        {
          devShells.default =
            (lib.bundles.tauriShell {
              name = "janis";
            }).overrideAttrs
              (old: {
                nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ shorthands ++ audioTools;
                buildInputs = (old.buildInputs or [ ]) ++ audioLibs ++ tauriLinuxLibs;
                shellHook = tauriLinuxShellEnv + (old.shellHook or "");
              });

          packages = {
            default = janis;
            inherit janis;
          };
        }
      ) workspace.lib;
    in
    {
      devShells = builtins.mapAttrs (_: ps: ps.devShells) perSystem;
      packages = builtins.mapAttrs (_: ps: ps.packages) perSystem;
    };
}
