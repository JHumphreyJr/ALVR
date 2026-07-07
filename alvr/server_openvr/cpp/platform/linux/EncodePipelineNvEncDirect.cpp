#ifdef ALVR_DIRECT_NVENC

#include "EncodePipelineNvEncDirect.h"

#include "ALVR-common/packet_types.h"
#include "Renderer.h"
#include "alvr_server/Logger.h"
#include "alvr_server/Settings.h"
#include "ffmpeg_helper.h"

#include <chrono>
#include <cstring>
#include <stdexcept>
#include <string>

// Not defined in the ffnvcodec dynlink headers
#ifndef CUDA_EXTERNAL_MEMORY_DEDICATED
#define CUDA_EXTERNAL_MEMORY_DEDICATED 0x1
#endif
#ifndef CUDA_ARRAY3D_SURFACE_LDST
#define CUDA_ARRAY3D_SURFACE_LDST 0x02
#endif
#ifndef CUDA_ARRAY3D_COLOR_ATTACHMENT
#define CUDA_ARRAY3D_COLOR_ATTACHMENT 0x20
#endif

namespace {

void cudaCheck(CudaFunctions* fns, CUresult res, const char* what) {
    if (res != CUDA_SUCCESS) {
        const char* msg = nullptr;
        fns->cuGetErrorString(res, &msg);
        throw std::runtime_error(
            std::string("CUDA error in ") + what + ": " + (msg ? msg : "unknown")
        );
    }
}

GUID codecGuid(ALVR_CODEC codec) {
    switch (codec) {
    case ALVR_CODEC_H264:
        return NV_ENC_CODEC_H264_GUID;
    case ALVR_CODEC_HEVC:
        return NV_ENC_CODEC_HEVC_GUID;
    case ALVR_CODEC_AV1:
        return NV_ENC_CODEC_AV1_GUID;
    }
    throw std::runtime_error("invalid codec " + std::to_string(codec));
}

GUID presetGuid(uint32_t qualityPreset) {
    switch (qualityPreset) {
    case 2:
        return NV_ENC_PRESET_P2_GUID;
    case 3:
        return NV_ENC_PRESET_P3_GUID;
    case 4:
        return NV_ENC_PRESET_P4_GUID;
    case 5:
        return NV_ENC_PRESET_P5_GUID;
    case 6:
        return NV_ENC_PRESET_P6_GUID;
    case 7:
        return NV_ENC_PRESET_P7_GUID;
    case 1:
    default:
        return NV_ENC_PRESET_P1_GUID;
    }
}

NV_ENC_TUNING_INFO tuningInfo(uint32_t tuningPreset) {
    // Same numbering as the FFmpeg nvenc "tune" option used by the settings schema
    switch (tuningPreset) {
    case 1:
        return NV_ENC_TUNING_INFO_HIGH_QUALITY;
    case 2:
        return NV_ENC_TUNING_INFO_LOW_LATENCY;
    case 3:
        return NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;
    case 4:
        return NV_ENC_TUNING_INFO_LOSSLESS;
    default:
        return NV_ENC_TUNING_INFO_LOW_LATENCY;
    }
}

} // namespace

alvr::EncodePipelineNvEncDirect::EncodePipelineNvEncDirect(
    Renderer* render, VkContext& vk_ctx, uint32_t width, uint32_t height
) {
    r = render;
    m_width = width;
    m_height = height;
    m_refreshRate = Settings::Instance().m_refreshRate;

    try {
        if (cuda_load_functions(&m_cuda, nullptr) < 0) {
            throw std::runtime_error("Failed to load libcuda (is the NVIDIA driver installed?)");
        }
        if (nvenc_load_functions(&m_nvencLib, nullptr) < 0) {
            throw std::runtime_error("Failed to load libnvidia-encode");
        }

        initCuda(vk_ctx);
        importVulkanOutput();
        initEncoder(width, height);
    } catch (...) {
        cleanup();
        throw;
    }
}

void alvr::EncodePipelineNvEncDirect::initCuda(VkContext& vk_ctx) {
    cudaCheck(m_cuda, m_cuda->cuInit(0), "cuInit");

    // Match the CUDA device to the Vulkan device by UUID
    VkPhysicalDeviceVulkan11Properties props11 = {};
    props11.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_1_PROPERTIES;
    VkPhysicalDeviceProperties2 props = {};
    props.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2;
    props.pNext = &props11;
    vkGetPhysicalDeviceProperties2(vk_ctx.get_vk_phys_device(), &props);

    int deviceCount = 0;
    cudaCheck(m_cuda, m_cuda->cuDeviceGetCount(&deviceCount), "cuDeviceGetCount");

    bool found = false;
    for (int i = 0; i < deviceCount; ++i) {
        CUdevice device;
        cudaCheck(m_cuda, m_cuda->cuDeviceGet(&device, i), "cuDeviceGet");

        CUuuid uuid;
        if (m_cuda->cuDeviceGetUuid(&uuid, device) != CUDA_SUCCESS) {
            continue;
        }
        if (memcmp(uuid.bytes, props11.deviceUUID, VK_UUID_SIZE) == 0) {
            m_cuDevice = device;
            found = true;
            break;
        }
    }
    if (!found) {
        if (deviceCount == 0) {
            throw std::runtime_error("No CUDA devices found");
        }
        Warn("No CUDA device matching the Vulkan device UUID, using device 0");
        cudaCheck(m_cuda, m_cuda->cuDeviceGet(&m_cuDevice, 0), "cuDeviceGet");
    }

    cudaCheck(
        m_cuda,
        m_cuda->cuDevicePrimaryCtxRetain(&m_cuContext, m_cuDevice),
        "cuDevicePrimaryCtxRetain"
    );
}

void alvr::EncodePipelineNvEncDirect::importVulkanOutput() {
    auto& output = r->GetOutput();

    if (output.imageInfo.format != VK_FORMAT_B8G8R8A8_UNORM
        && output.imageInfo.format != VK_FORMAT_R8G8B8A8_UNORM) {
        throw std::runtime_error(
            "Unsupported output format for direct NVENC: "
            + std::to_string(output.imageInfo.format)
        );
    }
    // NV_ENC_BUFFER_FORMAT_ARGB is B8G8R8A8 in memory, ABGR is R8G8B8A8
    m_bufferFormat = output.imageInfo.format == VK_FORMAT_B8G8R8A8_UNORM
        ? NV_ENC_BUFFER_FORMAT_ARGB
        : NV_ENC_BUFFER_FORMAT_ABGR;

    // Export the compositor output memory (allocated with OPAQUE_FD export on NVIDIA)
    VkMemoryGetFdInfoKHR memoryGetFdInfo = {};
    memoryGetFdInfo.sType = VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR;
    memoryGetFdInfo.memory = output.memory;
    memoryGetFdInfo.handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT;

    int fd = -1;
    VkResult res = r->d.vkGetMemoryFdKHR(r->m_dev, &memoryGetFdInfo, &fd);
    if (res != VK_SUCCESS || fd < 0) {
        throw std::runtime_error("vkGetMemoryFdKHR failed: " + Renderer::result_to_str(res));
    }

    m_cuda->cuCtxPushCurrent(m_cuContext);

    try {
        CUDA_EXTERNAL_MEMORY_HANDLE_DESC memDesc = {};
        memDesc.type = CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD;
        memDesc.handle.fd = fd;
        memDesc.size = output.size;
        memDesc.flags = CUDA_EXTERNAL_MEMORY_DEDICATED;

        // On success CUDA takes ownership of the fd
        cudaCheck(
            m_cuda, m_cuda->cuImportExternalMemory(&m_extMemory, &memDesc), "cuImportExternalMemory"
        );

        CUDA_EXTERNAL_MEMORY_MIPMAPPED_ARRAY_DESC arrayDesc = {};
        arrayDesc.offset = 0;
        arrayDesc.numLevels = 1;
        arrayDesc.arrayDesc.Width = output.imageInfo.extent.width;
        arrayDesc.arrayDesc.Height = output.imageInfo.extent.height;
        arrayDesc.arrayDesc.Depth = 0;
        arrayDesc.arrayDesc.Format = CU_AD_FORMAT_UNSIGNED_INT8;
        arrayDesc.arrayDesc.NumChannels = 4;
        arrayDesc.arrayDesc.Flags = CUDA_ARRAY3D_SURFACE_LDST | CUDA_ARRAY3D_COLOR_ATTACHMENT;

        cudaCheck(
            m_cuda,
            m_cuda->cuExternalMemoryGetMappedMipmappedArray(&m_mipArray, m_extMemory, &arrayDesc),
            "cuExternalMemoryGetMappedMipmappedArray"
        );
        cudaCheck(
            m_cuda, m_cuda->cuMipmappedArrayGetLevel(&m_array, m_mipArray, 0), "cuMipmappedArrayGetLevel"
        );
    } catch (...) {
        CUcontext dummy;
        m_cuda->cuCtxPopCurrent(&dummy);
        throw;
    }

    CUcontext dummy;
    m_cuda->cuCtxPopCurrent(&dummy);
}

void alvr::EncodePipelineNvEncDirect::initEncoder(uint32_t width, uint32_t height) {
    const auto& settings = Settings::Instance();
    auto codec_id = ALVR_CODEC(settings.m_codec);

    m_nvenc.version = NV_ENCODE_API_FUNCTION_LIST_VER;
    NVENCSTATUS status = m_nvencLib->NvEncodeAPICreateInstance(&m_nvenc);
    if (status != NV_ENC_SUCCESS) {
        throw std::runtime_error("NvEncodeAPICreateInstance failed: " + std::to_string(status));
    }

    NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS sessionParams = {};
    sessionParams.version = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
    sessionParams.deviceType = NV_ENC_DEVICE_TYPE_CUDA;
    sessionParams.device = m_cuContext;
    sessionParams.apiVersion = NVENCAPI_VERSION;
    status = m_nvenc.nvEncOpenEncodeSessionEx(&sessionParams, &m_encoder);
    if (status != NV_ENC_SUCCESS) {
        m_encoder = nullptr;
        throw std::runtime_error("nvEncOpenEncodeSessionEx failed: " + std::to_string(status));
    }

    GUID encodeGuid = codecGuid(codec_id);
    GUID preset = presetGuid(settings.m_nvencQualityPreset);
    NV_ENC_TUNING_INFO tuning = tuningInfo(settings.m_nvencTuningPreset);

    NV_ENC_PRESET_CONFIG presetConfig = {};
    presetConfig.version = NV_ENC_PRESET_CONFIG_VER;
    presetConfig.presetCfg.version = NV_ENC_CONFIG_VER;
    status = m_nvenc.nvEncGetEncodePresetConfigEx(
        m_encoder, encodeGuid, preset, tuning, &presetConfig
    );
    if (status != NV_ENC_SUCCESS) {
        throw std::runtime_error("nvEncGetEncodePresetConfigEx failed: " + std::to_string(status));
    }

    m_encodeConfig = presetConfig.presetCfg;
    m_encodeConfig.version = NV_ENC_CONFIG_VER;

    m_encodeConfig.gopLength = NVENC_INFINITE_GOPLENGTH;
    m_encodeConfig.frameIntervalP = 1; // no B frames

    auto& rc = m_encodeConfig.rcParams;
    rc.rateControlMode
        = settings.m_rateControlMode == ALVR_VBR ? NV_ENC_PARAMS_RC_VBR : NV_ENC_PARAMS_RC_CBR;
    rc.zeroReorderDelay = 1;
    rc.enableLookahead = 0;
    switch (settings.m_nvencMultiPass) {
    case 1:
        rc.multiPass = NV_ENC_TWO_PASS_QUARTER_RESOLUTION;
        break;
    case 2:
        rc.multiPass = NV_ENC_TWO_PASS_FULL_RESOLUTION;
        break;
    default:
        rc.multiPass = NV_ENC_MULTI_PASS_DISABLED;
        break;
    }
    if (settings.m_nvencAdaptiveQuantizationMode == 1) {
        rc.enableAQ = 1;
    } else if (settings.m_nvencAdaptiveQuantizationMode == 2) {
        rc.enableTemporalAQ = 1;
    }

    // Initial rate control values; updated dynamically through SetParams()
    rc.averageBitRate = 30'000'000;
    rc.maxBitRate = rc.averageBitRate;
    rc.vbvBufferSize = rc.averageBitRate / m_refreshRate * 1.1;
    rc.vbvInitialDelay = rc.vbvBufferSize / 4 * 3;

    switch (codec_id) {
    case ALVR_CODEC_H264: {
        auto& h264 = m_encodeConfig.encodeCodecConfig.h264Config;
        h264.repeatSPSPPS = 1;
        h264.idrPeriod = NVENC_INFINITE_GOPLENGTH;
        h264.entropyCodingMode = settings.m_entropyCoding == ALVR_CAVLC
            ? NV_ENC_H264_ENTROPY_CODING_MODE_CAVLC
            : NV_ENC_H264_ENTROPY_CODING_MODE_CABAC;
        switch (settings.m_h264Profile) {
        case ALVR_H264_PROFILE_BASELINE:
            m_encodeConfig.profileGUID = NV_ENC_H264_PROFILE_BASELINE_GUID;
            break;
        case ALVR_H264_PROFILE_MAIN:
            m_encodeConfig.profileGUID = NV_ENC_H264_PROFILE_MAIN_GUID;
            break;
        default:
        case ALVR_H264_PROFILE_HIGH:
            m_encodeConfig.profileGUID = NV_ENC_H264_PROFILE_HIGH_GUID;
            break;
        }
        if (settings.m_useFullRangeEncoding) {
            h264.h264VUIParameters.videoSignalTypePresentFlag = 1;
            h264.h264VUIParameters.videoFullRangeFlag = 1;
        }
        break;
    }
    case ALVR_CODEC_HEVC: {
        auto& hevc = m_encodeConfig.encodeCodecConfig.hevcConfig;
        hevc.repeatSPSPPS = 1;
        hevc.idrPeriod = NVENC_INFINITE_GOPLENGTH;
        if (settings.m_use10bitEncoder) {
            Warn("Direct NVENC pipeline encodes 8-bit (10-bit requires a 10-bit input surface)");
        }
        if (settings.m_useFullRangeEncoding) {
            hevc.hevcVUIParameters.videoSignalTypePresentFlag = 1;
            hevc.hevcVUIParameters.videoFullRangeFlag = 1;
        }
        break;
    }
    case ALVR_CODEC_AV1: {
        auto& av1 = m_encodeConfig.encodeCodecConfig.av1Config;
        av1.repeatSeqHdr = 1;
        av1.idrPeriod = NVENC_INFINITE_GOPLENGTH;
        break;
    }
    }

    m_initParams = {};
    m_initParams.version = NV_ENC_INITIALIZE_PARAMS_VER;
    m_initParams.encodeGUID = encodeGuid;
    m_initParams.presetGUID = preset;
    m_initParams.tuningInfo = tuning;
    m_initParams.encodeWidth = width;
    m_initParams.encodeHeight = height;
    m_initParams.darWidth = width;
    m_initParams.darHeight = height;
    m_initParams.maxEncodeWidth = width;
    m_initParams.maxEncodeHeight = height;
    m_initParams.frameRateNum = m_refreshRate;
    m_initParams.frameRateDen = 1;
    m_initParams.enablePTD = 1;
    m_initParams.enableEncodeAsync = 0;
    m_initParams.enableWeightedPrediction = settings.m_nvencEnableWeightedPrediction ? 1 : 0;
    m_initParams.encodeConfig = &m_encodeConfig;

    status = m_nvenc.nvEncInitializeEncoder(m_encoder, &m_initParams);
    if (status != NV_ENC_SUCCESS) {
        throw std::runtime_error(
            std::string("nvEncInitializeEncoder failed: ")
            + m_nvenc.nvEncGetLastErrorString(m_encoder)
        );
    }

    NV_ENC_CREATE_BITSTREAM_BUFFER bitstreamParams = {};
    bitstreamParams.version = NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
    status = m_nvenc.nvEncCreateBitstreamBuffer(m_encoder, &bitstreamParams);
    if (status != NV_ENC_SUCCESS) {
        throw std::runtime_error("nvEncCreateBitstreamBuffer failed: " + std::to_string(status));
    }
    m_bitstreamBuffer = bitstreamParams.bitstreamBuffer;

    NV_ENC_REGISTER_RESOURCE registerParams = {};
    registerParams.version = NV_ENC_REGISTER_RESOURCE_VER;
    registerParams.resourceType = NV_ENC_INPUT_RESOURCE_TYPE_CUDAARRAY;
    registerParams.width = m_width;
    registerParams.height = m_height;
    registerParams.pitch = m_width * 4;
    registerParams.resourceToRegister = (void*)m_array;
    registerParams.bufferFormat = m_bufferFormat;
    registerParams.bufferUsage = NV_ENC_INPUT_IMAGE;
    status = m_nvenc.nvEncRegisterResource(m_encoder, &registerParams);
    if (status != NV_ENC_SUCCESS) {
        throw std::runtime_error(
            std::string("nvEncRegisterResource failed: ")
            + m_nvenc.nvEncGetLastErrorString(m_encoder)
        );
    }
    m_registeredResource = registerParams.registeredResource;
}

alvr::EncodePipelineNvEncDirect::~EncodePipelineNvEncDirect() { cleanup(); }

void alvr::EncodePipelineNvEncDirect::cleanup() {
    if (m_encoder) {
        if (m_mappedResource) {
            m_nvenc.nvEncUnmapInputResource(m_encoder, m_mappedResource);
            m_mappedResource = nullptr;
        }
        if (m_registeredResource) {
            m_nvenc.nvEncUnregisterResource(m_encoder, m_registeredResource);
            m_registeredResource = nullptr;
        }
        if (m_bitstreamBuffer) {
            m_nvenc.nvEncDestroyBitstreamBuffer(m_encoder, m_bitstreamBuffer);
            m_bitstreamBuffer = nullptr;
        }
        m_nvenc.nvEncDestroyEncoder(m_encoder);
        m_encoder = nullptr;
    }

    if (m_cuda) {
        if (m_cuContext) {
            m_cuda->cuCtxPushCurrent(m_cuContext);
            if (m_mipArray) {
                m_cuda->cuMipmappedArrayDestroy(m_mipArray);
                m_mipArray = nullptr;
            }
            if (m_extMemory) {
                m_cuda->cuDestroyExternalMemory(m_extMemory);
                m_extMemory = nullptr;
            }
            CUcontext dummy;
            m_cuda->cuCtxPopCurrent(&dummy);
            m_cuda->cuDevicePrimaryCtxRelease(m_cuDevice);
            m_cuContext = nullptr;
        }
        cuda_free_functions(&m_cuda);
        m_cuda = nullptr;
    }
    if (m_nvencLib) {
        nvenc_free_functions(&m_nvencLib);
        m_nvencLib = nullptr;
    }
}

void alvr::EncodePipelineNvEncDirect::SetParams(FfiDynamicEncoderParams params) {
    if (!params.updated) {
        return;
    }

    auto& rc = m_encodeConfig.rcParams;
    uint32_t bitrate = params.bitrate_bps / params.framerate * m_refreshRate;
    rc.averageBitRate = bitrate;
    rc.maxBitRate = bitrate;
    rc.vbvBufferSize = bitrate / m_refreshRate * 1.1;
    rc.vbvInitialDelay = rc.vbvBufferSize / 4 * 3;

    NV_ENC_RECONFIGURE_PARAMS reconfigure = {};
    reconfigure.version = NV_ENC_RECONFIGURE_PARAMS_VER;
    reconfigure.reInitEncodeParams = m_initParams;
    reconfigure.reInitEncodeParams.encodeConfig = &m_encodeConfig;
    reconfigure.resetEncoder = 0;
    reconfigure.forceIDR = 0;

    NVENCSTATUS status = m_nvenc.nvEncReconfigureEncoder(m_encoder, &reconfigure);
    if (status != NV_ENC_SUCCESS) {
        Warn("nvEncReconfigureEncoder failed: %s", m_nvenc.nvEncGetLastErrorString(m_encoder));
    }
}

void alvr::EncodePipelineNvEncDirect::PushFrame(uint64_t targetTimestampNs, bool idr) {
    // Wait for the compositor to finish writing the output image. NVENC reads the same GPU
    // memory directly, no copy involved.
    r->Sync();

    timestamp.cpu = std::chrono::duration_cast<std::chrono::nanoseconds>(
                        std::chrono::steady_clock::now().time_since_epoch()
    )
                        .count();

    m_pendingIdr = idr;
    m_pendingPts = targetTimestampNs;
    m_frameQueued = true;
}

bool alvr::EncodePipelineNvEncDirect::GetEncoded(FramePacket& packet) {
    if (!m_frameQueued) {
        return false;
    }
    m_frameQueued = false;

    m_cuda->cuCtxPushCurrent(m_cuContext);

    NV_ENC_MAP_INPUT_RESOURCE mapParams = {};
    mapParams.version = NV_ENC_MAP_INPUT_RESOURCE_VER;
    mapParams.registeredResource = m_registeredResource;
    NVENCSTATUS status = m_nvenc.nvEncMapInputResource(m_encoder, &mapParams);
    if (status != NV_ENC_SUCCESS) {
        CUcontext dummy;
        m_cuda->cuCtxPopCurrent(&dummy);
        throw std::runtime_error(
            std::string("nvEncMapInputResource failed: ")
            + m_nvenc.nvEncGetLastErrorString(m_encoder)
        );
    }
    m_mappedResource = mapParams.mappedResource;

    NV_ENC_PIC_PARAMS picParams = {};
    picParams.version = NV_ENC_PIC_PARAMS_VER;
    picParams.inputWidth = m_width;
    picParams.inputHeight = m_height;
    picParams.inputPitch = m_width * 4;
    picParams.inputBuffer = m_mappedResource;
    picParams.outputBitstream = m_bitstreamBuffer;
    picParams.bufferFmt = m_bufferFormat;
    picParams.pictureStruct = NV_ENC_PIC_STRUCT_FRAME;
    picParams.inputTimeStamp = m_pendingPts;
    if (m_pendingIdr) {
        picParams.encodePicFlags = NV_ENC_PIC_FLAG_FORCEIDR | NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
    }

    status = m_nvenc.nvEncEncodePicture(m_encoder, &picParams);
    if (status != NV_ENC_SUCCESS) {
        m_nvenc.nvEncUnmapInputResource(m_encoder, m_mappedResource);
        m_mappedResource = nullptr;
        CUcontext dummy;
        m_cuda->cuCtxPopCurrent(&dummy);
        throw std::runtime_error(
            std::string("nvEncEncodePicture failed: ") + m_nvenc.nvEncGetLastErrorString(m_encoder)
        );
    }

    NV_ENC_LOCK_BITSTREAM lockParams = {};
    lockParams.version = NV_ENC_LOCK_BITSTREAM_VER;
    lockParams.outputBitstream = m_bitstreamBuffer;
    lockParams.doNotWait = 0;
    status = m_nvenc.nvEncLockBitstream(m_encoder, &lockParams);
    if (status != NV_ENC_SUCCESS) {
        m_nvenc.nvEncUnmapInputResource(m_encoder, m_mappedResource);
        m_mappedResource = nullptr;
        CUcontext dummy;
        m_cuda->cuCtxPopCurrent(&dummy);
        throw std::runtime_error(
            std::string("nvEncLockBitstream failed: ") + m_nvenc.nvEncGetLastErrorString(m_encoder)
        );
    }

    m_packetData.assign(
        (uint8_t*)lockParams.bitstreamBufferPtr,
        (uint8_t*)lockParams.bitstreamBufferPtr + lockParams.bitstreamSizeInBytes
    );
    bool isIdr = lockParams.pictureType == NV_ENC_PIC_TYPE_IDR;

    m_nvenc.nvEncUnlockBitstream(m_encoder, m_bitstreamBuffer);
    m_nvenc.nvEncUnmapInputResource(m_encoder, m_mappedResource);
    m_mappedResource = nullptr;

    CUcontext dummy;
    m_cuda->cuCtxPopCurrent(&dummy);

    packet.data = m_packetData.data();
    packet.size = (int)m_packetData.size();
    packet.pts = m_pendingPts;
    packet.isIDR = isIdr;

    return true;
}

#endif // ALVR_DIRECT_NVENC
