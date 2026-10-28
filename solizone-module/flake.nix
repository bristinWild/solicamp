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
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
