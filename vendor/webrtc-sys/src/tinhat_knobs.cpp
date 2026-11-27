// Tin Hat (12.2.2, share encoder effort): process-wide knobs the app sets from Rust and the NVIDIA encoders read at
// InitEncode. Always compiled (the NVIDIA sources are not, on a build without CUDA), so the Rust side links everywhere.
#include <atomic>

namespace {
// 0 = quality (the shipped share config: P5, LOW_LATENCY, two-pass quarter resolution, spatial + temporal AQ),
// 1 = light (P1, LOW_LATENCY, one pass, no AQ). Screen shares only; a camera always gets the quality config.
std::atomic<int> g_share_encoder_effort{0};
}  // namespace

extern "C" void tinhat_set_share_encoder_effort(int effort) {
  g_share_encoder_effort.store(effort == 1 ? 1 : 0, std::memory_order_relaxed);
}

extern "C" int tinhat_share_encoder_effort() {
  return g_share_encoder_effort.load(std::memory_order_relaxed);
}
