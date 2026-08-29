{
  description = "janis — devShell composed from chess-flake bundles";

  # The workspace owns the toolchain pins (Rust channel, Node version, pnpm,
  # prettier) and the shared shell shapes. Janis is not a chess project, but
  # the tauriShell bundle is domain-agnostic: Rust + Node + macOS pkg-config.
  inputs.workspace.url = "github:obazin/chess-flake";

  outputs =
    { self, workspace }:
    {
      devShells = builtins.mapAttrs (
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
        in
        {
          default =
            (lib.bundles.tauriShell {
              name = "janis";
            }).overrideAttrs
              (old: {
                nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ shorthands ++ audioTools;
                buildInputs = (old.buildInputs or [ ]) ++ audioLibs;
              });
        }
      ) workspace.lib;
    };
}
