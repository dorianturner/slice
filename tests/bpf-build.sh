#!/usr/bin/env bash
set -euo pipefail

slice_bpf_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
slice_bpf_output=$(mktemp --suffix=.bpf.o)
trap 'rm -f "$slice_bpf_output"' EXIT

slice_bpf_clang=${BPF_CLANG:-clang}
slice_bpf_cflags=()
slice_multiarch=$(cc -print-multiarch 2>/dev/null || true)
if [[ -n "$slice_multiarch" && -d "/usr/include/$slice_multiarch" ]]; then
  # `clang -target bpf` does not add the host multiarch include directory on
  # Ubuntu, but linux/types.h includes asm/types.h from there.
  slice_bpf_cflags+=("-I/usr/include/$slice_multiarch")
fi
if [[ -n "${BPF_CFLAGS:-}" ]]; then
  read -r -a slice_extra_bpf_cflags <<<"$BPF_CFLAGS"
  slice_bpf_cflags+=("${slice_extra_bpf_cflags[@]}")
fi
# Nix's pkg-config supplies the libbpf header path; linuxHeaders is present in
# the flake shell for <linux/bpf.h>.
"$slice_bpf_clang" -target bpf -O2 -g -Wall -Werror \
  "${slice_bpf_cflags[@]}" \
  $(pkg-config --cflags libbpf) \
  -c "$slice_bpf_root/crates/slice-ebpf/bpf/slice.bpf.c" -o "$slice_bpf_output"

test -s "$slice_bpf_output"
