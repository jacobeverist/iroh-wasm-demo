# Non-flake fallback: `nix-shell`.
#
# flake.nix is the better-pinned option (it pins rustc via rust-overlay and
# nixpkgs to nixos-unstable). This file just uses whatever channel you have, so
# it needs a reasonably recent nixpkgs:
#
#   * rustc >= 1.85            -- the crate is edition 2024
#   * wasm-bindgen-cli must match the version in Cargo.lock (build.sh checks)
#
# On a channel without the per-version attributes, `wasm-bindgen-cli_0_2_126`
# will not resolve; see the NixOS section of README.md for the options.

{ pkgs ? import <nixpkgs> { } }:

let
  # `ring` compiles C for wasm32; the nixpkgs cc-wrapper's injected host flags
  # break that, so hand cc-rs the unwrapped compiler.
  wasmCC = "${pkgs.llvmPackages.clang-unwrapped}/bin/clang";
  wasmAR = "${pkgs.llvmPackages.llvm}/bin/llvm-ar";
in
pkgs.mkShell {
  packages = [
    pkgs.rustc
    pkgs.cargo
    pkgs.wasm-bindgen-cli_0_2_126
    pkgs.binaryen
    pkgs.python3
    pkgs.llvmPackages.clang-unwrapped
    pkgs.llvmPackages.llvm
  ];

  CC_wasm32_unknown_unknown = wasmCC;
  AR_wasm32_unknown_unknown = wasmAR;

  # See flake.nix: hardening flags produce symbols wasm cannot satisfy, which
  # surface in the browser as "Relative references must start with ./".
  hardeningDisable = [ "all" ];
  CFLAGS_wasm32_unknown_unknown = "-fno-stack-protector";
}
