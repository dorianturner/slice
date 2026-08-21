use std::{env, ffi::OsString, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=bpf/slice.bpf.c");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let clang = env::var("BPF_CLANG").unwrap_or_else(|_| "clang".to_owned());
    let mut builder = libbpf_cargo::SkeletonBuilder::new();
    builder.source("bpf/slice.bpf.c").clang(clang);
    let mut clang_args = Vec::<OsString>::new();
    if let Ok(output) = Command::new("cc").arg("-print-multiarch").output() {
        if output.status.success() {
            let multiarch = String::from_utf8_lossy(&output.stdout);
            let include = PathBuf::from("/usr/include").join(multiarch.trim());
            if include.is_dir() {
                clang_args.push(format!("-I{}", include.display()).into());
            }
        }
    }
    if let Ok(flags) = env::var("BPF_CFLAGS") {
        clang_args.extend(flags.split_whitespace().map(OsString::from));
    }
    builder.clang_args(clang_args);
    builder
        .build_and_generate(output.join("slice.skel.rs"))
        .expect("failed to build the Slice eBPF skeleton");
}
