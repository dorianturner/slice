#include <cstdlib>
#include <iostream>

// Select work() here to validate the collector's non-overlap guard. Nested
// selected invocations are deliberately outside the POC contract and must be
// recorded as invalid rather than silently double-associated with samples.
namespace SliceFixture {
[[gnu::noinline]] unsigned work(unsigned depth) {
  volatile unsigned value = depth + 1;
  if (depth != 0) value += work(depth - 1);
  return value;
}
}  // namespace SliceFixture

int main(int argc, char** argv) {
  const unsigned depth = argc > 1 ? static_cast<unsigned>(std::strtoul(argv[1], nullptr, 10)) : 4U;
  std::cout << "nested-population value=" << SliceFixture::work(depth) << '\n';
}

