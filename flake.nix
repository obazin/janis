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
          # `run` — launch the app in dev (`just run` → `pnpm tauri dev`).
          # A wrapper on PATH rather than a shell alias so it works in zsh,
          # bash, `nix develop` and non-interactive shells alike.
          shorthands = [
            (lib.pkgs.writeShellScriptBin "run" ''
              exec just run "$@"
            '')
          ];
        in
        {
          default =
            (lib.bundles.tauriShell {
              name = "janis";
            }).overrideAttrs
              (old: {
                nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ shorthands;
              });
        }
      ) workspace.lib;
    };
}
