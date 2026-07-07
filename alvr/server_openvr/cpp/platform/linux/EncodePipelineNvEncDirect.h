#pragma once

#ifdef ALVR_DIRECT_NVENC

#include "EncodePipeline.h"

#include <cstdint>
#include <vector>

extern "C" {
#include <ffnvcodec/dynlink_cuda.h>
#include <ffnvcodec/dynlink_loader.h>
#include <ffnvcodec/nvEncodeAPI.h>
}

class Renderer;

namespace alvr {

class VkContext;

// Zero-copy NVENC pipeline: the compositor output image (Vulkan, optimal tiling) is imported
// into CUDA as an external memory mipmapped array and registered with NVENC directly. No
// per-frame Vulkan->CUDA transfer and no FFmpeg involvement on the hot path.
class EncodePipelineNvEncDirect : public EncodePipeline {
public:
    EncodePipelineNvEncDirect(Renderer* render, VkContext& vk_ctx, uint32_t width, uint32_t height);
    ~EncodePipelineNvEncDirect() override;

    void PushFrame(uint64_t targetTimestampNs, bool idr) override;
    bool GetEncoded(FramePacket& packet) override;
    void SetParams(FfiDynamicEncoderParams params) override;

private:
    void initCuda(VkContext& vk_ctx);
    void importVulkanOutput();
    void initEncoder(uint32_t width, uint32_t height);
    void cleanup();

    Renderer* r = nullptr;

    CudaFunctions* m_cuda = nullptr;
    NvencFunctions* m_nvencLib = nullptr;
    NV_ENCODE_API_FUNCTION_LIST m_nvenc = {};

    CUdevice m_cuDevice = 0;
    CUcontext m_cuContext = nullptr;

    CUexternalMemory m_extMemory = nullptr;
    CUmipmappedArray m_mipArray = nullptr;
    CUarray m_array = nullptr;

    void* m_encoder = nullptr;
    NV_ENC_REGISTERED_PTR m_registeredResource = nullptr;
    NV_ENC_INPUT_PTR m_mappedResource = nullptr;
    NV_ENC_OUTPUT_PTR m_bitstreamBuffer = nullptr;
    NV_ENC_BUFFER_FORMAT m_bufferFormat = NV_ENC_BUFFER_FORMAT_UNDEFINED;

    NV_ENC_INITIALIZE_PARAMS m_initParams = {};
    NV_ENC_CONFIG m_encodeConfig = {};

    uint32_t m_width = 0;
    uint32_t m_height = 0;
    uint32_t m_refreshRate = 60;

    bool m_frameQueued = false;
    bool m_pendingIdr = false;
    uint64_t m_pendingPts = 0;

    std::vector<uint8_t> m_packetData;
};

}

#endif // ALVR_DIRECT_NVENC
