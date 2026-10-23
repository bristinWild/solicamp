# solicamp-ui

Native QML front end for SoliCamp (Basecamp `ui_qml` module). It calls the
`solizone_module` core module through `logos.callModuleAsync`.

Edit `flake.nix` so the `solizone_module` input points at your solizone-module
repo, then `git add -A && nix build .#lgx` and load both packages in Basecamp.