#include <algorithm>
#include <atomic>
#include <bit>
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
// centered around 10ms and 20ms, each with roughly 5ms of normal variation,
// plus the real distribution work performed for every request.
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

// Keep the distribution work visible to statistical stack sampling. The
// standard-library operator is implemented in a header and is commonly
// inlined, so calling it once would make it effectively unobservable in a
// real profile. This wrapper still executes the real distribution repeatedly;
// it does not manufacture a stack frame or profile data.
[[gnu::noinline]] std::chrono::microseconds normal_distribution(
    std::mt19937& generator, double mean_us, double deviation_us,
    double minimum_us) {
  std::normal_distribution<double> distribution{mean_us, deviation_us};
  double selected = distribution(generator);
  double checksum = selected;
  for (unsigned sample = 0; sample < 65'536; ++sample) {
    checksum += distribution(generator);
  }
  sink.fetch_xor(std::bit_cast<std::uint64_t>(checksum),
                std::memory_order_relaxed);
  return std::chrono::microseconds{static_cast<std::int64_t>(
      std::max(minimum_us, selected))};
}

[[gnu::noinline]] void fast_path(std::mt19937& generator) {
  spin_for(normal_distribution(generator, 10'000.0, 5'000.0, 1'000.0));
}

// std::this_thread::sleep_for is an inline standard-library wrapper and may
// not have a symbol of its own. Keep a stable application frame on the blocked
// user stack so the README's off-CPU view has a portable, inspectable label.
[[gnu::noinline]] void sleep_for(std::chrono::microseconds duration) {
  std::this_thread::sleep_for(duration);
}

[[gnu::noinline]] void slow_path(std::mt19937& generator) {
  // Keep a stable CPU component so the slow mode remains useful for the
  // off-CPU view; the wait and spin carry the requested 20ms +/- 5ms shape,
  // in addition to the real distribution work.
  sleep_for(normal_distribution(generator, 15'000.0, 5'000.0, 1'000.0));
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
