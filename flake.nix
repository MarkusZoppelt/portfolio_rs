{
  description = "A command line tool with interactive TUI for managing financial investment portfolios written in Rust";

  nixConfig = {
    extra-substituters = [
      "https://nix-community.cachix.org"
    ];
    extra-trusted-public-keys = [
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    crane,
    flake-utils,
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = nixpkgs.legacyPackages.${system};
        craneLib = crane.mkLib pkgs;

        commonArgs = {
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              (craneLib.filterCargoSources path type)
              || (pkgs.lib.hasSuffix ".json" path);
          };
          strictDeps = true;

          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs = with pkgs;
            [
              openssl
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              libiconv
            ];
        };

        # Build just the dependencies (for faster incremental builds)
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Build the actual package
        portfolio_rs = craneLib.buildPackage (commonArgs
          // {
            inherit cargoArtifacts;

            doCheck = false; # Tests are run via the separate portfolio_rs-test check

            meta = with pkgs.lib; {
              description = "Command line tool for managing financial investment portfolios";
              homepage = "https://github.com/MarkusZoppelt/portfolio_rs";
              license = licenses.mit;
              maintainers = [maintainers.MarkusZoppelt];
              mainProgram = "portfolio_rs";
            };
          });
      in {
        packages = {
          default = portfolio_rs;
          portfolio_rs = portfolio_rs;
        };

        # App for `nix run`
        apps.default = flake-utils.lib.mkApp {
          drv = portfolio_rs;
        };

        # Development shell
        devShells.default = craneLib.devShell {
          packages = with pkgs; [
            # Rust tooling
            rust-analyzer
            rustfmt
            clippy
            cargo-watch
            cargo-audit

            # Formatters
            alejandra # Nix formatter

            # Build dependencies
            pkg-config
            openssl
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            libiconv
          ];
        };

        # Checks for `nix flake check`
        checks = {
          inherit portfolio_rs;

          portfolio_rs-clippy = craneLib.cargoClippy (commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            });

          portfolio_rs-fmt = craneLib.cargoFmt {
            inherit (commonArgs) src;
          };

          portfolio_rs-test = craneLib.cargoTest (commonArgs
            // {
              inherit cargoArtifacts;
            });
        };

        # Nix formatter for `nix fmt`
        formatter = pkgs.alejandra;
      }
    )
    // {
      # Overlay for easy integration
      overlays.default = final: prev: {
        portfolio_rs = self.packages.${final.system}.portfolio_rs;
      };
    };
}
