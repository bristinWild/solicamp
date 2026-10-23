{
  description = "Solizone EVM - Logos core module (Rust) that runs a Solizone node inside Basecamp";

  inputs = {
    # The 0.3 generation: what today's Basecamp loads (0.2 modules don't).
    logos-module-builder.url = "github:logos-co/logos-module-builder/0.3.2";
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
