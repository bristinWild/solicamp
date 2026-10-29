logos-rust-sdk a55fdac (0.2.0), vendored because logos-blockchain-module's Zone SDK backend
depends on it. Only change: src/plugin.rs announces a declared module origin (set_module_origin)
instead of the literal "core". Remove when the backend moves to a current SDK.
