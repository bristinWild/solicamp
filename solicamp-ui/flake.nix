{
  description = "SoliCamp - Solidity playground UI (QML) for Basecamp";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/0.3.2";

    # The core module this UI calls. Lives in the same repo (bristinWild/solicamp), in the solizone-module folder.
    solizone_module = {
      url = "github:bristinWild/solicamp?dir=solizone-module";
      inputs.logos-module-builder.follows = "logos-module-builder";
    };
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosQmlModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
