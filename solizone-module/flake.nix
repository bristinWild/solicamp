{
  description = "Solizone EVM - Logos core module (Rust) that runs a Solizone node inside Basecamp";

  # Prebuilt delivery_module / liblogosdelivery come from the Logos binary
  # cache. Without it Nix would compile the Nim messaging library from source
  # (a long build). Nix asks once whether to trust this; answer yes, or pass
  # --accept-flake-config.
  nixConfig = {
    extra-substituters = [ "https://cache.nix.logos.co/public" ];
    extra-trusted-public-keys = [ "public:l4HrXgL4nw246+LBh2SOJyhz64BoGegOYLheT/iIAPU=" ];
  };

  inputs = {
    # The 0.3 generation: what today's Basecamp loads (0.2 modules don't).
    logos-module-builder.url = "github:logos-co/logos-module-builder/0.3.2";

    # Logos Messaging: carries shared-room traffic between people. The input
    # name must equal the name in metadata.json "dependencies".
    delivery_module.url = "github:logos-co/logos-delivery-module/v0.3.0";

    # Prebuilt ZK libraries the Logos Zone SDK's crates link ("Publish to
    # Bedrock"). Their build scripts would otherwise download them, which the
    # Nix sandbox forbids. Same pins as the Logos node's own flake; keep them in
    # sync with the lbc-* tag and the rust-rapidsnark rev in its Cargo.toml.
    logos-blockchain-circuits.url = "github:logos-blockchain/logos-blockchain-circuits/v0.5.7";
    rust-rapidsnark.url = "github:logos-blockchain/logos-blockchain-rust-rapidsnark/e91187f8ccb5bbfc7bb00dac88169112428da78f";
  };

  outputs = inputs@{ logos-module-builder, logos-blockchain-circuits, rust-rapidsnark, ... }:
    let
      # The ZK libraries differ per system, and the builder takes one `rustEnv`
      # for all of them, so build the module once per system with that system's
      # libraries and keep each system's own outputs.
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];

      # librocksdb-sys (a dependency of the node) runs bindgen, which needs
      # libclang and its headers. macOS provides them; on Linux the Nix build
      # sandbox does not, so point bindgen at them.
      clangEnv = system:
        let
          pkgs = import logos-module-builder.inputs.nixpkgs { inherit system; };
          clang = pkgs.llvmPackages.libclang;
        in
        if pkgs.stdenv.isLinux then {
          LIBCLANG_PATH = "${clang.lib}/lib";
          BINDGEN_EXTRA_CLANG_ARGS = builtins.concatStringsSep " " [
            "-isystem ${clang.lib}/lib/clang/${pkgs.lib.versions.major clang.version}/include"
            "-isystem ${pkgs.glibc.dev}/include"
          ];
        } else { };

      moduleFor = system: logos-module-builder.lib.mkLogosModule {
        src = ./.;
        configFile = ./metadata.json;
        flakeInputs = inputs;
        rustEnv = {
          RAPIDSNARK_LIB_DIR = "${rust-rapidsnark.packages.${system}.rapidsnark}";
          LBC_ROOT_DIR = "${logos-blockchain-circuits.packages.${system}.default}";
        } // clangEnv system;
      };

      built = builtins.listToAttrs (map (system: { name = system; value = moduleFor system; }) systems);

      perSystem = attr: builtins.listToAttrs (map
        (system: { name = system; value = built.${system}.${attr}.${system}; })
        systems);

      first = built.${builtins.head systems};
    in
    {
      packages = perSystem "packages";
      devShells = perSystem "devShells";
      configFor = perSystem "configFor";
      inherit (first) config metadataJson;
    };
}
