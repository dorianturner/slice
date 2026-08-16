#include <algorithm>
#include <atomic>
#include <chrono>
#include <csignal>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <random>
#include <string_view>
#include <thread>
#include <unistd.h>
#include <vector>

// This is the README end-to-end workload. It has two overlapping latency modes
// centered around 10ms and 20ms, each with roughly 5ms of normal variation.
namespace BimodalFixture {

std::atomic<bool> running{true};
std::atomic<std::uint64_t> next_request{0};
std::atomic<std::uint64_t> sink{0};
thread_local std::mt19937 generator{0x51ceU};

void on_signal(int) { running.store(false, std::memory_order_relaxed); }

[[gnu::noinline]] void spin_for(std::chrono::microseconds duration) {
  const auto deadline = std::chrono::steady_clock::now() + duration;
  std::uint64_t local = 0x9e3779b97f4a7c15ULL;
  while (std::chrono::steady_clock::now() < deadline) {
    local ^= local << 7;
    local ^= local >> 9;
    local *= 0x9e3779b97f4a7c15ULL;
  }
  sink.fetch_xor(local, std::memory_order_relaxed);
}

template <typename Distribution>
std::chrono::microseconds sample_duration(Distribution& distribution,
                                           std::mt19937& generator,
                                           double minimum_us) {
  return std::chrono::microseconds{static_cast<std::int64_t>(std::max(
      minimum_us, distribution(generator)))};
}

[[gnu::noinline]] void fast_path(std::mt19937& generator) {
  std::normal_distribution<double> duration_us{10'000.0, 5'000.0};
  spin_for(sample_duration(duration_us, generator, 1'000.0));
}

[[gnu::noinline]] void slow_path(std::mt19937& generator) {
  // Keep a stable CPU component so the slow mode remains useful for the
  // off-CPU view; the wait carries the requested 20ms +/- 5ms total shape.
  std::normal_distribution<double> wait_us{15'000.0, 5'000.0};
  std::this_thread::sleep_for(sample_duration(wait_us, generator, 1'000.0));
  spin_for(std::chrono::microseconds{5'000});
}

[[gnu::noinline]] void handle_request(std::uint64_t request_id) {
  // Seven fast calls followed by three slow calls keeps the broad 70/30 split.
  if (request_id % 10U < 7U) {
    fast_path(generator);
  } else {
    slow_path(generator);
  }
}

}  // namespace BimodalFixture

int main(int argc, char** argv) {
  unsigned workers = 4;
  std::uint64_t iterations = 0;
  for (int index = 1; index < argc; ++index) {
    const std::string_view argument(argv[index]);
    if (argument == "--workers" && index + 1 < argc) {
      workers = static_cast<unsigned>(std::strtoul(argv[++index], nullptr, 10));
    } else if (argument == "--iterations" && index + 1 < argc) {
      iterations = std::strtoull(argv[++index], nullptr, 10);
    } else if (argument == "--help") {
      std::cout << "usage: bimodal_service [--workers N] [--iterations N]\n";
      return 0;
    }
  }
  if (workers == 0) workers = 1;
  std::signal(SIGINT, BimodalFixture::on_signal);
  std::signal(SIGTERM, BimodalFixture::on_signal);
  std::cout << "bimodal_service pid=" << ::getpid()
            << " workers=" << workers
            << " modes=70% fast(~10ms +/- 5ms),30% slow(~20ms +/- 5ms)\n"
            << std::flush;

  std::vector<std::thread> threads;
  threads.reserve(workers);
  for (unsigned worker = 0; worker < workers; ++worker) {
    threads.emplace_back([iterations, worker] {
      BimodalFixture::generator.seed(0x51ce'0000U + worker);
      std::uint64_t completed = 0;
      while (BimodalFixture::running.load(std::memory_order_relaxed)
             && (iterations == 0 || completed < iterations)) {
        const auto request_id = BimodalFixture::next_request.fetch_add(
            1, std::memory_order_relaxed);
        BimodalFixture::handle_request(request_id);
        ++completed;
      }
    });
  }
  for (auto& thread : threads) thread.join();
  std::cout << "bimodal_service stopped sink="
            << BimodalFixture::sink.load(std::memory_order_relaxed) << '\n';
}
