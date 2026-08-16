{
  description = "Slice — percentile-conditioned C++ profiler POC";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      system = "x86_64-linux";
      overlays = [ (import rust-overlay) ];
      pkgs = import nixpkgs { inherit system overlays; };
      rustToolchain = pkgs.rust-bin.stable.latest.default.override {
        extensions = [ "rust-src" "rustfmt" "clippy" ];
        targets = [ "wasm32-unknown-unknown" ];
      };
      nativeBuildInputs = with pkgs; [
        rustToolchain
        clang
        llvmPackages.bintools
        pkg-config
        zlib
        elfutils
        linuxHeaders
        libbpf
        bpftools
        nodejs_22
        pnpm
        wasm-bindgen-cli
        binaryen
        cargo-nextest
        cmake
        ninja
        just
      ];
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = nativeBuildInputs;
        # The wrapped compiler injects host-only hardening flags that clang's
        # BPF backend rejects; BPF objects must use the unwrapped binary.
        BPF_CLANG = "${pkgs.llvmPackages.clang-unwrapped}/bin/clang";
        BPF_CFLAGS = "-I${pkgs.linuxHeaders}/include -I${pkgs.glibc.dev}/include -I${pkgs.libbpf}/include";
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
      };

      packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
        pname = "slice";
        version = "0.1.0";
        src = self;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [
          pkgs.pkg-config
          pkgs.clang
          pkgs.llvmPackages.bintools
        ];
        buildInputs = [ pkgs.elfutils pkgs.zlib pkgs.libbpf ];
        BPF_CLANG = "${pkgs.llvmPackages.clang-unwrapped}/bin/clang";
        BPF_CFLAGS = "-I${pkgs.linuxHeaders}/include -I${pkgs.glibc.dev}/include -I${pkgs.libbpf}/include";
        doCheck = true;
        checkPhase = ''
          runHook preCheck
          cargo test --workspace --offline
          runHook postCheck
        '';
      };

      apps.${system}.default = {
        type = "app";
        program = "${self.packages.${system}.default}/bin/slice";
        meta.description = "Slice percentile-conditioned profiler";
      };

      checks.${system} = {
        unit = self.packages.${system}.default;
        bpf-compile = pkgs.runCommand "slice-bpf-compile" { } ''
          ${pkgs.llvmPackages.clang-unwrapped}/bin/clang -target bpf -O2 -g -Wall -Werror \
            -I${pkgs.linuxHeaders}/include -I${pkgs.glibc.dev}/include \
            -I${pkgs.libbpf}/include \
            -c ${self}/crates/slice-ebpf/bpf/slice.bpf.c -o slice.bpf.o
          test -s slice.bpf.o
          touch $out
        '';
      };
    };
}
