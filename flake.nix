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
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    crane,
    rust-overlay,
    flake-utils,
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay)];
        };
        # The latest stable Rust as of flake.lock, so the weekly lock update
        # moves the compiler. The package and the checks get a minimal
        # toolchain; the dev shell adds rust-analyzer, rust-src and llvm-tools
        # (for `cargo coverage`, which CI runs in the dev shell) on the same
        # version.
        craneLib = (crane.mkLib pkgs).overrideToolchain (p:
          p.rust-bin.stable.latest.minimal.override {
            extensions = ["clippy" "rustfmt"];
          });
        devCraneLib = craneLib.overrideToolchain (p:
          p.rust-bin.stable.latest.default.override {
            extensions = ["llvm-tools-preview" "rust-analyzer" "rust-src"];
          });

        commonArgs = {
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              (craneLib.filterCargoSources path type)
              || (pkgs.lib.hasSuffix ".json" path)
              || (pkgs.lib.hasSuffix ".md" path)
              || (pkgs.lib.hasSuffix ".snap" path);
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
        devShells.default = devCraneLib.devShell {
          packages = with pkgs;
            [
              # Rust tooling (cargo, rustc, clippy, rustfmt and
              # rust-analyzer come from the toolchain above)
              cargo-watch
              cargo-audit
              cargo-llvm-cov

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
              # The doctor tests build a real git repository.
              nativeBuildInputs = commonArgs.nativeBuildInputs ++ [pkgs.git];
              # assert_cmd::cargo_bin resolves the binary via this env var,
              # which `cargo test` sets but crane's cargoTest does not.
              # Point it at the Nix-built binary so e2e_offline tests pass.
              preCheck = ''
                export CARGO_BIN_EXE_portfolio_rs="${portfolio_rs}/bin/portfolio_rs"
                # confy writes config to $HOME/.config on startup; give it a
                # writable home so the sandbox doesn't break every e2e test.
                export HOME=$(mktemp -d)
              '';
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
