{
  description = "janis — devShell + app package, composed from chess-flake bundles";

  # The workspace owns the toolchain pins (Rust channel, Node version, pnpm,
  # prettier) and the shared shell shapes. Janis is not a chess project, but
  # the rustShell bundle is domain-agnostic: the Rust toolchain and its tools.
  inputs.workspace.url = "github:obazin/chess-flake";

  outputs =
    { self, workspace }:
    let
      perSystem = builtins.mapAttrs (
        system: lib:
        let
          pkgs = lib.pkgs;

          # `run` — launch the app in dev (`just run` → `cargo run`).
          # A wrapper on PATH rather than a shell alias so it works in zsh,
          # bash, `nix develop` and non-interactive shells alike.
          shorthands = [
            (pkgs.writeShellScriptBin "run" ''
              exec just run "$@"
            '')
          ];

          # The audio engine needs more than the bundle's Rust.
          #
          # cmake: the `opus` crate builds libopus from bundled source via
          # `opusic-sys`, which drives cmake. Without it the Opus decoder —
          # and so the whole backend — does not compile.
          audioTools = [ pkgs.cmake ];

          # alsa-lib: cpal's Linux backend links ALSA through pkg-config.
          # macOS reaches CoreAudio through the SDK and needs nothing here.
          audioLibs = pkgs.lib.optionals pkgs.stdenv.isLinux [ pkgs.alsa-lib ];

          # What the window needs on Linux: fonts, keyboard layouts, Wayland
          # and X11, and OpenGL for the renderer. Most of these are loaded at
          # run time rather than linked, so the shell and the package both
          # have to say where they are. macOS needs nothing here.
          windowLibs =
            with pkgs;
            pkgs.lib.optionals pkgs.stdenv.isLinux [
              fontconfig
              libGL
              libxkbcommon
              wayland
              xorg.libX11
              xorg.libXcursor
              xorg.libXi
              xorg.libXrandr
              xorg.libxcb
            ];

          windowShellEnv = pkgs.lib.optionalString pkgs.stdenv.isLinux ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath windowLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
          '';

          # Keep `version` in sync with Cargo.toml.
          # Built with the workspace's Rust toolchain rather than nixpkgs' own,
          # which trails behind what the interface toolkit needs.
          rustPlatform = pkgs.makeRustPlatform {
            cargo = lib.rustToolchain;
            rustc = lib.rustToolchain;
          };

          janis = rustPlatform.buildRustPackage (finalAttrs: {
            pname = "janis";
            version = "0.1.0";

            src = ./.;

            cargoHash = "sha256-e8V0Rk3ru4nRolGb5u58hHIhnpGO2NoUNvAIGeKxe0c=";

            nativeBuildInputs = [
              pkgs.cmake
              pkgs.pkg-config
            ];

            buildInputs = windowLibs ++ audioLibs;

            # The quality gates are the local suites (`just check`); some of
            # them need a loopback socket the build sandbox does not give.
            doCheck = false;

            # The binary finds the libraries it loads at run time.
            postFixup = pkgs.lib.optionalString pkgs.stdenv.isLinux ''
              patchelf --add-rpath "${pkgs.lib.makeLibraryPath windowLibs}" $out/bin/Janis
            '';

            # On macOS the binary lives in an application bundle, so the system
            # shows Janis with its name and icon, and `bin/Janis` points into
            # it. On Linux, the desktop entry and icon are installed.
            postInstall =
              if pkgs.stdenv.isDarwin then
                ''
                  bundle="$out/Applications/Janis.app/Contents"
                  mkdir -p "$bundle/MacOS" "$bundle/Resources"
                  substitute packaging/macos/Info.plist "$bundle/Info.plist" \
                    --subst-var-by VERSION ${finalAttrs.version}
                  cp packaging/icons/icon.icns "$bundle/Resources/icon.icns"
                  mv "$out/bin/Janis" "$bundle/MacOS/Janis"
                  ln -s "$bundle/MacOS/Janis" "$out/bin/Janis"
                ''
              else
                ''
                  install -Dm644 packaging/linux/janis.desktop $out/share/applications/janis.desktop
                  install -Dm644 packaging/icons/128x128.png $out/share/icons/hicolor/128x128/apps/janis.png
                '';

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
            (lib.bundles.rustShell {
              name = "janis";
            }).overrideAttrs
              (old: {
                nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ shorthands ++ audioTools;
                buildInputs = (old.buildInputs or [ ]) ++ audioLibs ++ windowLibs;
                # Prepended: the bundle's own hook exec's into zsh, so anything
                # appended after it would never run.
                shellHook = windowShellEnv + (old.shellHook or "");
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

      # `nixpkgs.overlays = [ janis.overlays.default ]` puts `pkgs.janis` in a
      # NixOS, nix-darwin or Home Manager configuration.
      overlays.default = final: _prev: {
        janis = self.packages.${final.stdenv.hostPlatform.system}.janis;
      };
    };
}
