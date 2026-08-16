#include <chrono>
#include <cstdlib>
#include <iostream>
#include <thread>

// A compact off-CPU validation target: sleep_work() alternates a tiny CPU
// section and a scheduler-visible wait. The collector should attribute the
// waiting interval to an off-CPU sample of this invocation.
namespace SliceFixture {
[[gnu::noinline]] void cpu_prelude() {
  volatile unsigned value = 0;
  for (unsigned i = 0; i < 10000; ++i) value += i;
}

[[gnu::noinline]] void sleep_work(unsigned milliseconds) {
  cpu_prelude();
  std::this_thread::sleep_for(std::chrono::milliseconds(milliseconds));
  cpu_prelude();
}
}  // namespace SliceFixture

int main(int argc, char** argv) {
  const unsigned calls = argc > 1 ? static_cast<unsigned>(std::strtoul(argv[1], nullptr, 10)) : 8U;
  for (unsigned i = 0; i < calls; ++i) SliceFixture::sleep_work(20U + (i % 2U) * 20U);
  std::cout << "off-cpu-wait calls=" << calls << '\n';
}

