#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <thread>
#include <vector>

// This workload proves Slice's core claim. Across every 100 work() calls,
// fast_aggregate_a and slow_tail_b each consume 297ms of aggregate CPU time.
// A conventional aggregate flame graph therefore gives them equal width. The
// single p99 call, however, contains only slow_tail_b.
namespace SliceFixture {
namespace {
std::atomic<std::uint64_t> sink{0};

[[gnu::noinline]] void spin_for(std::chrono::milliseconds duration) {
  const auto deadline = std::chrono::steady_clock::now() + duration;
  std::uint64_t local = 0;
  while (std::chrono::steady_clock::now() < deadline) {
    local = local * 1664525U + 1013904223U;
  }
  sink.fetch_add(local, std::memory_order_relaxed);
}
}  // namespace

[[gnu::noinline]] void fast_aggregate_a() { spin_for(std::chrono::milliseconds{3}); }
[[gnu::noinline]] void slow_tail_b() { spin_for(std::chrono::milliseconds{297}); }

[[gnu::noinline]] void work(unsigned iteration) {
  if (iteration % 100U == 99U) {
    slow_tail_b();
  } else {
    fast_aggregate_a();
  }
}
}  // namespace SliceFixture

int main(int argc, char** argv) {
  const unsigned iterations = argc > 1 ? static_cast<unsigned>(std::strtoul(argv[1], nullptr, 10)) : 100U;
  const unsigned workers = argc > 2 ? static_cast<unsigned>(std::strtoul(argv[2], nullptr, 10)) : 1U;
  std::vector<std::thread> threads;
  threads.reserve(workers);
  for (unsigned worker = 0; worker < workers; ++worker) {
    threads.emplace_back([=] {
      for (unsigned iteration = 0; iteration < iterations; ++iteration) {
        SliceFixture::work(iteration);
      }
    });
  }
  for (auto& thread : threads) thread.join();
  std::cout << "tail-divergence workers=" << workers << " iterations=" << iterations << " sink=" << SliceFixture::sink.load() << '\n';
}

