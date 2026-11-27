#ifndef WEBRTC_SYS_NVIDIA_CUDA_CONTEXT_H
#define WEBRTC_SYS_NVIDIA_CUDA_CONTEXT_H

#include <cuda.h>

#include <mutex>

namespace livekit_ffi {

class CudaContext {
 public:
  CudaContext() = default;
  ~CudaContext() = default;

  static bool IsAvailable();

  static CudaContext* GetInstance();
  bool Initialize();
  bool IsInitialized() const { return cu_context_ != nullptr; }
  CUcontext GetContext() const;

  // PATCH (Tin Hat 12.2.2): reference-counted use. Every NVENC encoder takes a reference when the factory creates it
  // and drops it in its destructor; the context is created by the first reference and DESTROYED with the last. Until
  // 12.2.1 the first encoder created the context for the life of the process, and any CUDA context, even an idle one,
  // holds an NVIDIA card in P1 (memory clock 14001 -> 13365 MHz measured on an RTX PRO 6000): a share or a GPU camera
  // once kept it there until the app exited. TINHAT_CUDA_KEEP=1 keeps the context once made (the old behaviour).
  // nullptr = no context (the error is logged).
  CUcontext Acquire();
  void ReleaseRef();
  // Diagnostics: references held now, and contexts created/destroyed so far (the gate reads them from the log).
  int refs();

  void Shutdown();

 private:
  CUdevice cu_device_ = 0;
  CUcontext cu_context_ = nullptr;
  std::mutex mu_;
  int refs_ = 0;
};

}  // namespace livekit_ffi

#endif  // WEBRTC_SYS_NVIDIA_CUDA_CONTEXT_H
