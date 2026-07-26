{
  description = "iroh compiled to WebAssembly, driven from the browser by plain JS";

  inputs = {
    # wasm-bindgen-cli is packaged per-version (wasm-bindgen-cli_0_2_126 etc.)
    # in nixos-unstable, which is what lets us match Cargo.lock exactly. Older
    # channels ship only a single `wasm-bindgen-cli` at whatever version they
    # froze at, which will usually NOT match -- see README.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # Gives us a rustc that actually has the wasm32-unknown-unknown std,
    # pinned independently of whatever the channel's rustc happens to be.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };

          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            targets = [ "wasm32-unknown-unknown" ];
          };

          # `ring` compiles C for wasm32. Use the UNWRAPPED clang: the nixpkgs
          # cc-wrapper injects host-specific flags and a host -target, which
          # break the cross-compile. ring passes -nostdlibinc and defines
          # RING_CORE_NOSTDLIBINC, so it needs no libc headers -- an unwrapped
          # compiler is sufficient as well as necessary.
          wasmCC = "${pkgs.llvmPackages.clang-unwrapped}/bin/clang";
          wasmAR = "${pkgs.llvmPackages.llvm}/bin/llvm-ar";

          # MUST equal the wasm-bindgen version in Cargo.lock. If you bump the
          # crate, bump this attribute too; build.sh compares them and fails
          # loudly rather than emitting subtly broken glue.
          wasmBindgen = pkgs.wasm-bindgen-cli_0_2_126;
        in
        {
          default = pkgs.mkShell {
            packages = [
              rustToolchain
              wasmBindgen
              pkgs.binaryen # wasm-opt, for ./build.sh --release
              pkgs.python3 # ./serve.sh
              pkgs.llvmPackages.clang-unwrapped
              pkgs.llvmPackages.llvm
            ];

            # build.sh honours these if already set and skips its own probing.
            CC_wasm32_unknown_unknown = wasmCC;
            AR_wasm32_unknown_unknown = wasmAR;

            shellHook = ''
              echo "iroh-wasm-demo dev shell"
              echo "  rustc         $(rustc --version 2>/dev/null)"
              echo "  wasm-bindgen  $(wasm-bindgen --version 2>/dev/null)"
              echo "  wasm cc       ${wasmCC}"
              echo
              echo "  ./build.sh && ./serve.sh"
            '';
          };
        }
      );
    };
}
