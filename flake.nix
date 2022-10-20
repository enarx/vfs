{
  inputs.nixify.url = github:rvolosatovs/nixify;

  outputs = {nixify, ...}:
    nixify.lib.rust.mkFlake {
      name = "vfs";
      src = ./.;

      ignorePaths = [
        "/.github"
        "/.gitignore"
        "/flake.lock"
        "/flake.nix"
        "/rust-toolchain.toml"
      ];

      targets.wasm32-wasi = false; # wasi-common fails to compile for wasi

      test.allFeatures = true;
      test.allTargets = true;
      test.noDefaultFeatures = false;
      test.workspace = true;

      clippy.allFeatures = true;
      clippy.allTargets = true;
      clippy.deny = ["warnings"];
      clippy.noDefaultFeatures = false;
      clippy.workspace = true;
    };
}
