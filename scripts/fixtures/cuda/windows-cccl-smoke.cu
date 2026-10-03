// Compile only: this fixture must never launch a kernel or require a GPU.
#include <cuda/std/type_traits>
#include <cooperative_groups.h>
#include <cooperative_groups/reduce.h>

#if !defined(_MSC_VER) || !defined(_MSVC_TRADITIONAL) || _MSVC_TRADITIONAL != 0
#error "Windows CUDA builds require /Zc:preprocessor"
#endif

static_assert(cuda::std::is_same_v<int, int>);

__global__ void ooosplat_cccl_compile_probe(float* values) {
  const auto group = cooperative_groups::tiled_partition<32>(
      cooperative_groups::this_thread_block());
  values[threadIdx.x] = cooperative_groups::reduce(
      group, float(threadIdx.x), cooperative_groups::plus<float>());
}
