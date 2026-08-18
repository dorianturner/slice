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
      rustToolchain = pkgs.rust-bin.stable."1.85.1".default.override {
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
        cargo-deny
        actionlint
        gitleaks
        jq
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
        native-fixtures = pkgs.runCommand "slice-native-fixtures" {
          nativeBuildInputs = [ pkgs.cmake pkgs.ninja pkgs.stdenv.cc ];
        } ''
          cmake -S ${self}/fixtures -B build -G Ninja -DCMAKE_BUILD_TYPE=RelWithDebInfo
          cmake --build build
          ctest --test-dir build --output-on-failure
          ${self.packages.${system}.default}/bin/slice symbols build/bimodal_service --match handle_request \
            | grep -F $'\tBimodalFixture::handle_request(unsigned long)'
          ${self.packages.${system}.default}/bin/slice fixture-profile --scenario bimodal --output bimodal.slice
          ${self.packages.${system}.default}/bin/slice view bimodal.slice --output bimodal.html --percentile 95:100
          grep -F 'id="timeline"' bimodal.html
          grep -F 'BimodalFixture::slow_path()' bimodal.html
          touch $out
        '';
      };
    };
}
