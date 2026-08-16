#!/usr/bin/env bash
set -euo pipefail

slice_bpf_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
slice_bpf_output=$(mktemp --suffix=.bpf.o)
trap 'rm -f "$slice_bpf_output"' EXIT

slice_bpf_clang=${BPF_CLANG:-clang}
# Nix's pkg-config supplies the libbpf header path; linuxHeaders is present in
# the flake shell for <linux/bpf.h>.
"$slice_bpf_clang" -target bpf -O2 -g -Wall -Werror \
  ${BPF_CFLAGS:-} \
  $(pkg-config --cflags libbpf) \
  -c "$slice_bpf_root/crates/slice-ebpf/bpf/slice.bpf.c" -o "$slice_bpf_output"

test -s "$slice_bpf_output"
