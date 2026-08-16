use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=bpf/slice.bpf.c");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let clang = env::var("BPF_CLANG").unwrap_or_else(|_| "clang".to_owned());
    let mut builder = libbpf_cargo::SkeletonBuilder::new();
    builder.source("bpf/slice.bpf.c").clang(clang);
    if let Ok(flags) = env::var("BPF_CFLAGS") {
        builder.clang_args(flags.split_whitespace());
    }
    builder
        .build_and_generate(output.join("slice.skel.rs"))
        .expect("failed to build the Slice eBPF skeleton");
}
