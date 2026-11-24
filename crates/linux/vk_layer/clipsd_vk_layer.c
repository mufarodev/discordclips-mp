/* Dormant implicit Vulkan DMA-BUF capture layer for clipsd (self-contained, libc+pthread only). */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

typedef uint32_t VkFlags;
typedef uint32_t VkBool32;
typedef uint64_t VkDeviceSize;
typedef int32_t  VkResult;

typedef struct VkInstance_T       *VkInstance;
typedef struct VkPhysicalDevice_T *VkPhysicalDevice;
typedef struct VkDevice_T         *VkDevice;
typedef struct VkQueue_T          *VkQueue;
typedef struct VkCommandBuffer_T  *VkCommandBuffer;

typedef uint64_t VkSurfaceKHR;
typedef uint64_t VkSwapchainKHR;
typedef uint64_t VkImage;
typedef uint64_t VkDeviceMemory;
typedef uint64_t VkCommandPool;
typedef uint64_t VkSemaphore;
typedef uint64_t VkFence;

#define VK_SUCCESS 0
#define VK_NOT_READY 1
#define VK_TIMEOUT 2
#define VK_SUBOPTIMAL_KHR 1000001003
#define VK_NULL_HANDLE 0

#define VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO                         1
#define VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO                           3
#define VK_STRUCTURE_TYPE_SUBMIT_INFO                                  4
#define VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO                         5
#define VK_STRUCTURE_TYPE_FENCE_CREATE_INFO                            8
#define VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO                        9
#define VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO                            14
#define VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO                     39
#define VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO                 40
#define VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO                    42
#define VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER                         45
#define VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO                  47
#define VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO                    48
#define VK_STRUCTURE_TYPE_FORMAT_PROPERTIES_2                          1000059008
#define VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO            1000072001
#define VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO                  1000072002
#define VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR                       1000074002
#define VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO               1000127001
#define VK_STRUCTURE_TYPE_DRM_FORMAT_MODIFIER_PROPERTIES_LIST_EXT      1000158000
#define VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_LIST_CREATE_INFO_EXT 1000158003
#define VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_PROPERTIES_EXT     1000158005
#define VK_STRUCTURE_TYPE_PRESENT_INFO_KHR                             1000001001

#define VK_FORMAT_B8G8R8A8_UNORM 44
#define VK_IMAGE_TYPE_2D 1
#define VK_IMAGE_TILING_OPTIMAL 0
#define VK_IMAGE_TILING_DRM_FORMAT_MODIFIER_EXT 1000158000
#define VK_SHARING_MODE_EXCLUSIVE 0
#define VK_SAMPLE_COUNT_1_BIT 1
#define VK_FILTER_NEAREST 0

#define VK_IMAGE_USAGE_TRANSFER_SRC_BIT 0x00000001
#define VK_IMAGE_USAGE_TRANSFER_DST_BIT 0x00000002
#define VK_IMAGE_USAGE_SAMPLED_BIT      0x00000004
#define VK_FORMAT_FEATURE_TRANSFER_DST_BIT 0x00008000

#define VK_IMAGE_LAYOUT_UNDEFINED            0
#define VK_IMAGE_LAYOUT_GENERAL              1
#define VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL 6
#define VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL 7
#define VK_IMAGE_LAYOUT_PRESENT_SRC_KHR      1000001002

#define VK_IMAGE_ASPECT_COLOR_BIT          0x00000001
#define VK_IMAGE_ASPECT_MEMORY_PLANE_0_BIT 0x00000080

#define VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT 0x00000001
#define VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT 0x00000200

#define VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT    0x00000001
#define VK_PIPELINE_STAGE_TRANSFER_BIT       0x00001000
#define VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT 0x00002000
#define VK_ACCESS_TRANSFER_READ_BIT          0x00000800
#define VK_ACCESS_TRANSFER_WRITE_BIT         0x00001000
#define VK_QUEUE_FAMILY_IGNORED              (~0U)

#define VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT 0x00000002
#define VK_COMMAND_BUFFER_LEVEL_PRIMARY                 0
#define VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT     0x00000001
#define VK_FENCE_CREATE_SIGNALED_BIT                    0x00000001

#define DRM_FORMAT_ARGB8888 0x34325241U
#define DRM_FORMAT_MOD_INVALID 0x00ffffffffffffffULL

typedef void (*PFN_vkVoidFunction)(void);
typedef PFN_vkVoidFunction (*PFN_vkGetInstanceProcAddr)(VkInstance, const char *);
typedef PFN_vkVoidFunction (*PFN_vkGetDeviceProcAddr)(VkDevice, const char *);

typedef struct { uint32_t sType; const void *pNext; } VkBaseInStructure;
typedef struct { uint32_t sType; void *pNext; } VkBaseOutStructure;

typedef struct VkNegotiateLayerInterface {
    uint32_t sType;
    void *pNext;
    uint32_t loaderLayerInterfaceVersion;
    PFN_vkGetInstanceProcAddr pfnGetInstanceProcAddr;
    PFN_vkGetDeviceProcAddr pfnGetDeviceProcAddr;
    PFN_vkVoidFunction pfnGetPhysicalDeviceProcAddr;
} VkNegotiateLayerInterface;

typedef struct VkLayerInstanceLink {
    struct VkLayerInstanceLink *pNext;
    PFN_vkGetInstanceProcAddr pfnNextGetInstanceProcAddr;
    PFN_vkVoidFunction pfnNextGetPhysicalDeviceProcAddr;
} VkLayerInstanceLink;

typedef struct VkLayerInstanceCreateInfo {
    uint32_t sType;
    const void *pNext;
    uint32_t function;
    union {
        VkLayerInstanceLink *pLayerInfo;
        void *other;
    } u;
} VkLayerInstanceCreateInfo;

typedef struct VkLayerDeviceLink {
    struct VkLayerDeviceLink *pNext;
    PFN_vkGetInstanceProcAddr pfnNextGetInstanceProcAddr;
    PFN_vkGetDeviceProcAddr pfnNextGetDeviceProcAddr;
} VkLayerDeviceLink;

typedef struct VkLayerDeviceCreateInfo {
    uint32_t sType;
    const void *pNext;
    uint32_t function;
    union {
        VkLayerDeviceLink *pLayerInfo;
        void *other;
    } u;
} VkLayerDeviceCreateInfo;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags flags;
    const void *pApplicationInfo;
    uint32_t enabledLayerCount;
    const char *const *ppEnabledLayerNames;
    uint32_t enabledExtensionCount;
    const char *const *ppEnabledExtensionNames;
} VkInstanceCreateInfo;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags flags;
    uint32_t queueCreateInfoCount;
    const void *pQueueCreateInfos;
    uint32_t enabledLayerCount;
    const char *const *ppEnabledLayerNames;
    uint32_t enabledExtensionCount;
    const char *const *ppEnabledExtensionNames;
    const void *pEnabledFeatures;
} VkDeviceCreateInfo;

typedef struct {
    char extensionName[256];
    uint32_t specVersion;
} VkExtensionProperties;

typedef struct { uint32_t width, height; } VkExtent2D;
typedef struct { uint32_t width, height, depth; } VkExtent3D;
typedef struct { int32_t x, y, z; } VkOffset3D;

typedef struct {
    uint32_t minImageCount;
    uint32_t maxImageCount;
    VkExtent2D currentExtent;
    VkExtent2D minImageExtent;
    VkExtent2D maxImageExtent;
    uint32_t maxImageArrayLayers;
    VkFlags supportedTransforms;
    VkFlags currentTransform;
    VkFlags supportedCompositeAlpha;
    VkFlags supportedUsageFlags;
} VkSurfaceCapabilitiesKHR;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags flags;
    VkSurfaceKHR surface;
    uint32_t minImageCount;
    uint32_t imageFormat;
    uint32_t imageColorSpace;
    VkExtent2D imageExtent;
    uint32_t imageArrayLayers;
    VkFlags imageUsage;
    uint32_t imageSharingMode;
    uint32_t queueFamilyIndexCount;
    const uint32_t *pQueueFamilyIndices;
    uint32_t preTransform;
    uint32_t compositeAlpha;
    uint32_t presentMode;
    VkBool32 clipped;
    VkSwapchainKHR oldSwapchain;
} VkSwapchainCreateInfoKHR;

typedef struct {
    uint32_t sType;
    const void *pNext;
    uint32_t waitSemaphoreCount;
    const VkSemaphore *pWaitSemaphores;
    uint32_t swapchainCount;
    const VkSwapchainKHR *pSwapchains;
    const uint32_t *pImageIndices;
    VkResult *pResults;
} VkPresentInfoKHR;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags flags;
    uint32_t queueFamilyIndex;
    uint32_t queueIndex;
} VkDeviceQueueInfo2;

typedef struct {
    VkFlags propertyFlags;
    uint32_t heapIndex;
} VkMemoryType;

typedef struct {
    VkDeviceSize size;
    VkFlags flags;
} VkMemoryHeap;

typedef struct {
    uint32_t memoryTypeCount;
    VkMemoryType memoryTypes[32];
    uint32_t memoryHeapCount;
    VkMemoryHeap memoryHeaps[16];
} VkPhysicalDeviceMemoryProperties;

typedef struct {
    VkFlags linearTilingFeatures;
    VkFlags optimalTilingFeatures;
    VkFlags bufferFeatures;
} VkFormatProperties;

typedef struct {
    uint32_t sType;
    void *pNext;
    VkFormatProperties formatProperties;
} VkFormatProperties2;

typedef struct {
    uint64_t drmFormatModifier;
    uint32_t drmFormatModifierPlaneCount;
    VkFlags drmFormatModifierTilingFeatures;
} VkDrmFormatModifierPropertiesEXT;

typedef struct {
    uint32_t sType;
    void *pNext;
    uint32_t drmFormatModifierCount;
    VkDrmFormatModifierPropertiesEXT *pDrmFormatModifierProperties;
} VkDrmFormatModifierPropertiesListEXT;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags handleTypes;
} VkExternalMemoryImageCreateInfo;

typedef struct {
    uint32_t sType;
    const void *pNext;
    uint32_t drmFormatModifierCount;
    const uint64_t *pDrmFormatModifiers;
} VkImageDrmFormatModifierListCreateInfoEXT;

typedef struct {
    uint32_t sType;
    void *pNext;
    uint64_t drmFormatModifier;
} VkImageDrmFormatModifierPropertiesEXT;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags flags;
    uint32_t imageType;
    uint32_t format;
    VkExtent3D extent;
    uint32_t mipLevels;
    uint32_t arrayLayers;
    uint32_t samples;
    uint32_t tiling;
    VkFlags usage;
    uint32_t sharingMode;
    uint32_t queueFamilyIndexCount;
    const uint32_t *pQueueFamilyIndices;
    uint32_t initialLayout;
} VkImageCreateInfo;

typedef struct {
    VkDeviceSize size;
    VkDeviceSize alignment;
    uint32_t memoryTypeBits;
} VkMemoryRequirements;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags handleTypes;
} VkExportMemoryAllocateInfo;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkImage image;
    uint64_t buffer;
} VkMemoryDedicatedAllocateInfo;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkDeviceSize allocationSize;
    uint32_t memoryTypeIndex;
} VkMemoryAllocateInfo;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkDeviceMemory memory;
    uint32_t handleType;
} VkMemoryGetFdInfoKHR;

typedef struct {
    VkFlags aspectMask;
    uint32_t mipLevel;
    uint32_t arrayLayer;
} VkImageSubresource;

typedef struct {
    VkDeviceSize offset;
    VkDeviceSize size;
    VkDeviceSize rowPitch;
    VkDeviceSize arrayPitch;
    VkDeviceSize depthPitch;
} VkSubresourceLayout;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags flags;
    uint32_t queueFamilyIndex;
} VkCommandPoolCreateInfo;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkCommandPool commandPool;
    uint32_t level;
    uint32_t commandBufferCount;
} VkCommandBufferAllocateInfo;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags flags;
    const void *pInheritanceInfo;
} VkCommandBufferBeginInfo;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags flags;
} VkSemaphoreCreateInfo;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags flags;
} VkFenceCreateInfo;

typedef struct {
    VkFlags aspectMask;
    uint32_t baseMipLevel;
    uint32_t levelCount;
    uint32_t baseArrayLayer;
    uint32_t layerCount;
} VkImageSubresourceRange;

typedef struct {
    uint32_t sType;
    const void *pNext;
    VkFlags srcAccessMask;
    VkFlags dstAccessMask;
    uint32_t oldLayout;
    uint32_t newLayout;
    uint32_t srcQueueFamilyIndex;
    uint32_t dstQueueFamilyIndex;
    VkImage image;
    VkImageSubresourceRange subresourceRange;
} VkImageMemoryBarrier;

typedef struct {
    VkFlags aspectMask;
    uint32_t mipLevel;
    uint32_t baseArrayLayer;
    uint32_t layerCount;
} VkImageSubresourceLayers;

typedef struct {
    VkImageSubresourceLayers srcSubresource;
    VkOffset3D srcOffsets[2];
    VkImageSubresourceLayers dstSubresource;
    VkOffset3D dstOffsets[2];
} VkImageBlit;

typedef struct {
    uint32_t sType;
    const void *pNext;
    uint32_t waitSemaphoreCount;
    const VkSemaphore *pWaitSemaphores;
    const VkFlags *pWaitDstStageMask;
    uint32_t commandBufferCount;
    const VkCommandBuffer *pCommandBuffers;
    uint32_t signalSemaphoreCount;
    const VkSemaphore *pSignalSemaphores;
} VkSubmitInfo;

typedef VkResult (*PFN_vkCreateInstance)(const VkInstanceCreateInfo *, const void *, VkInstance *);
typedef void     (*PFN_vkDestroyInstance)(VkInstance, const void *);
typedef VkResult (*PFN_vkCreateDevice)(VkPhysicalDevice, const VkDeviceCreateInfo *, const void *, VkDevice *);
typedef void     (*PFN_vkDestroyDevice)(VkDevice, const void *);
typedef VkResult (*PFN_vkEnumerateDeviceExtensionProperties)(VkPhysicalDevice, const char *, uint32_t *, VkExtensionProperties *);
typedef void     (*PFN_vkGetPhysicalDeviceMemoryProperties)(VkPhysicalDevice, VkPhysicalDeviceMemoryProperties *);
typedef void     (*PFN_vkGetPhysicalDeviceFormatProperties2)(VkPhysicalDevice, uint32_t, VkFormatProperties2 *);
typedef VkResult (*PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR)(VkPhysicalDevice, VkSurfaceKHR, VkSurfaceCapabilitiesKHR *);
typedef void     (*PFN_vkGetDeviceQueue)(VkDevice, uint32_t, uint32_t, VkQueue *);
typedef void     (*PFN_vkGetDeviceQueue2)(VkDevice, const VkDeviceQueueInfo2 *, VkQueue *);
typedef VkResult (*PFN_vkCreateSwapchainKHR)(VkDevice, const VkSwapchainCreateInfoKHR *, const void *, VkSwapchainKHR *);
typedef void     (*PFN_vkDestroySwapchainKHR)(VkDevice, VkSwapchainKHR, const void *);
typedef VkResult (*PFN_vkGetSwapchainImagesKHR)(VkDevice, VkSwapchainKHR, uint32_t *, VkImage *);
typedef VkResult (*PFN_vkQueuePresentKHR)(VkQueue, const VkPresentInfoKHR *);
typedef VkResult (*PFN_vkCreateImage)(VkDevice, const VkImageCreateInfo *, const void *, VkImage *);
typedef void     (*PFN_vkDestroyImage)(VkDevice, VkImage, const void *);
typedef void     (*PFN_vkGetImageMemoryRequirements)(VkDevice, VkImage, VkMemoryRequirements *);
typedef VkResult (*PFN_vkAllocateMemory)(VkDevice, const VkMemoryAllocateInfo *, const void *, VkDeviceMemory *);
typedef void     (*PFN_vkFreeMemory)(VkDevice, VkDeviceMemory, const void *);
typedef VkResult (*PFN_vkBindImageMemory)(VkDevice, VkImage, VkDeviceMemory, VkDeviceSize);
typedef VkResult (*PFN_vkGetMemoryFdKHR)(VkDevice, const VkMemoryGetFdInfoKHR *, int *);
typedef VkResult (*PFN_vkGetImageDrmFormatModifierPropertiesEXT)(VkDevice, VkImage, VkImageDrmFormatModifierPropertiesEXT *);
typedef void     (*PFN_vkGetImageSubresourceLayout)(VkDevice, VkImage, const VkImageSubresource *, VkSubresourceLayout *);
typedef VkResult (*PFN_vkCreateCommandPool)(VkDevice, const VkCommandPoolCreateInfo *, const void *, VkCommandPool *);
typedef void     (*PFN_vkDestroyCommandPool)(VkDevice, VkCommandPool, const void *);
typedef VkResult (*PFN_vkAllocateCommandBuffers)(VkDevice, const VkCommandBufferAllocateInfo *, VkCommandBuffer *);
typedef VkResult (*PFN_vkBeginCommandBuffer)(VkCommandBuffer, const VkCommandBufferBeginInfo *);
typedef VkResult (*PFN_vkEndCommandBuffer)(VkCommandBuffer);
typedef VkResult (*PFN_vkResetCommandBuffer)(VkCommandBuffer, VkFlags);
typedef void     (*PFN_vkCmdPipelineBarrier)(VkCommandBuffer, VkFlags, VkFlags, VkFlags, uint32_t, const void *, uint32_t, const void *, uint32_t, const VkImageMemoryBarrier *);
typedef void     (*PFN_vkCmdBlitImage)(VkCommandBuffer, VkImage, uint32_t, VkImage, uint32_t, uint32_t, const VkImageBlit *, uint32_t);
typedef VkResult (*PFN_vkQueueSubmit)(VkQueue, uint32_t, const VkSubmitInfo *, VkFence);
typedef VkResult (*PFN_vkCreateSemaphore)(VkDevice, const VkSemaphoreCreateInfo *, const void *, VkSemaphore *);
typedef void     (*PFN_vkDestroySemaphore)(VkDevice, VkSemaphore, const void *);
typedef VkResult (*PFN_vkCreateFence)(VkDevice, const VkFenceCreateInfo *, const void *, VkFence *);
typedef void     (*PFN_vkDestroyFence)(VkDevice, VkFence, const void *);
typedef VkResult (*PFN_vkGetFenceStatus)(VkDevice, VkFence);
typedef VkResult (*PFN_vkWaitForFences)(VkDevice, uint32_t, const VkFence *, VkBool32, uint64_t);
typedef VkResult (*PFN_vkResetFences)(VkDevice, uint32_t, const VkFence *);
typedef VkResult (*PFN_vkDeviceWaitIdle)(VkDevice);

#define CLVK_MAGIC 0x434C564BU /* 'CLVK' */
#define CLVK_MSG_SETUP 1U
#define CLVK_MSG_FRAME 2U
#define NUM_SLOTS 2

typedef struct {
    uint32_t magic;
    uint32_t target_fps;
} ClvkHello;

typedef struct {
    uint32_t magic;
    uint32_t kind;
    uint32_t width;
    uint32_t height;
    uint32_t fourcc;
    uint32_t slot;
    uint64_t modifier;
    uint32_t stride[NUM_SLOTS];
    uint32_t offset[NUM_SLOTS];
    int64_t  pts_us;
} ClvkMsg;

static inline int64_t monotonic_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000LL + (int64_t)ts.tv_nsec / 1000LL;
}

static inline void *dispatch_key(const void *handle) {
    return *(void *const *)handle;
}

#define MAX_INSTANCES  16
#define MAX_DEVICES    32
#define MAX_QUEUES     128
#define MAX_SWAPCHAINS 32

typedef struct {
    void *key;
    VkInstance instance;
    PFN_vkGetInstanceProcAddr next_gipa;
    PFN_vkDestroyInstance DestroyInstance;
    PFN_vkEnumerateDeviceExtensionProperties EnumerateDeviceExtensionProperties;
    PFN_vkGetPhysicalDeviceMemoryProperties GetPhysicalDeviceMemoryProperties;
    PFN_vkGetPhysicalDeviceFormatProperties2 GetPhysicalDeviceFormatProperties2;
    PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR GetPhysicalDeviceSurfaceCapabilitiesKHR;
} InstData;

typedef struct {
    void *key;
    VkDevice device;
    VkPhysicalDevice phys;
    InstData *inst;
    bool has_dmabuf_ext;
    bool has_modifier_ext;
    PFN_vkGetDeviceProcAddr next_gdpa;
    PFN_vkDestroyDevice DestroyDevice;
    PFN_vkGetDeviceQueue GetDeviceQueue;
    PFN_vkGetDeviceQueue2 GetDeviceQueue2;
    PFN_vkCreateSwapchainKHR CreateSwapchainKHR;
    PFN_vkDestroySwapchainKHR DestroySwapchainKHR;
    PFN_vkGetSwapchainImagesKHR GetSwapchainImagesKHR;
    PFN_vkQueuePresentKHR QueuePresentKHR;
    PFN_vkCreateImage CreateImage;
    PFN_vkDestroyImage DestroyImage;
    PFN_vkGetImageMemoryRequirements GetImageMemoryRequirements;
    PFN_vkAllocateMemory AllocateMemory;
    PFN_vkFreeMemory FreeMemory;
    PFN_vkBindImageMemory BindImageMemory;
    PFN_vkGetMemoryFdKHR GetMemoryFdKHR;
    PFN_vkGetImageDrmFormatModifierPropertiesEXT GetImageDrmFormatModifierPropertiesEXT;
    PFN_vkGetImageSubresourceLayout GetImageSubresourceLayout;
    PFN_vkCreateCommandPool CreateCommandPool;
    PFN_vkDestroyCommandPool DestroyCommandPool;
    PFN_vkAllocateCommandBuffers AllocateCommandBuffers;
    PFN_vkBeginCommandBuffer BeginCommandBuffer;
    PFN_vkEndCommandBuffer EndCommandBuffer;
    PFN_vkResetCommandBuffer ResetCommandBuffer;
    PFN_vkCmdPipelineBarrier CmdPipelineBarrier;
    PFN_vkCmdBlitImage CmdBlitImage;
    PFN_vkQueueSubmit QueueSubmit;
    PFN_vkCreateSemaphore CreateSemaphore;
    PFN_vkDestroySemaphore DestroySemaphore;
    PFN_vkCreateFence CreateFence;
    PFN_vkDestroyFence DestroyFence;
    PFN_vkGetFenceStatus GetFenceStatus;
    PFN_vkWaitForFences WaitForFences;
    PFN_vkResetFences ResetFences;
    PFN_vkDeviceWaitIdle DeviceWaitIdle;
} DevData;

typedef struct {
    VkQueue queue;
    uint32_t family;
} QueueEntry;

typedef struct {
    VkImage image;
    VkDeviceMemory mem;
    int dmabuf_fd;
    uint32_t stride;
    uint32_t offset;
    VkCommandBuffer cmd;
    VkSemaphore sem;
    VkFence fence;
} CaptureSlot;

typedef struct {
    bool in_use;
    uint32_t slot;
    int64_t pts_us;
} PendingNotify;

typedef struct {
    VkSwapchainKHR swapchain;
    DevData *dev;
    uint32_t width;
    uint32_t height;
    uint32_t format;
    uint32_t image_count;
    VkImage *images;

    bool capture_ready;
    uint32_t queue_family;
    VkCommandPool cmd_pool;
    uint64_t modifier;
    CaptureSlot slots[NUM_SLOTS];
    uint32_t next_slot;
    int64_t next_due_us;
    uint32_t accept_poll_div;

    pthread_t notify_thread;
    bool notify_running;
    bool notify_stop;
    pthread_mutex_t notify_mu;
    pthread_cond_t notify_cv;
    PendingNotify notify_q[4];
    int notify_head;
    int notify_tail;
} SwapchainData;

static pthread_mutex_t g_mu = PTHREAD_MUTEX_INITIALIZER;
static InstData g_insts[MAX_INSTANCES];
static DevData  g_devs[MAX_DEVICES];
static QueueEntry g_queues[MAX_QUEUES];
static SwapchainData g_swaps[MAX_SWAPCHAINS];

static int g_listen_fd = -1;
static int g_client_fd = -1;
static uint32_t g_target_fps = 60;
static bool g_setup_sent_for_active = false;
static VkSwapchainKHR g_active_swap = VK_NULL_HANDLE;

static InstData *find_inst(const void *handle) {
    void *k = dispatch_key(handle);
    for (int i = 0; i < MAX_INSTANCES; i++) {
        if (g_insts[i].key == k) return &g_insts[i];
    }
    return NULL;
}

static DevData *find_dev(const void *handle) {
    void *k = dispatch_key(handle);
    for (int i = 0; i < MAX_DEVICES; i++) {
        if (g_devs[i].key == k) return &g_devs[i];
    }
    return NULL;
}

static uint32_t find_queue_family(VkQueue q) {
    for (int i = 0; i < MAX_QUEUES; i++) {
        if (g_queues[i].queue == q) return g_queues[i].family;
    }
    return 0;
}

static void record_queue_family(VkQueue q, uint32_t family) {
    for (int i = 0; i < MAX_QUEUES; i++) {
        if (g_queues[i].queue == q) { g_queues[i].family = family; return; }
    }
    for (int i = 0; i < MAX_QUEUES; i++) {
        if (g_queues[i].queue == NULL) { g_queues[i].queue = q; g_queues[i].family = family; return; }
    }
}

static SwapchainData *find_swap(VkSwapchainKHR sc) {
    for (int i = 0; i < MAX_SWAPCHAINS; i++) {
        if (g_swaps[i].swapchain == sc) return &g_swaps[i];
    }
    return NULL;
}

static void ensure_listener_locked(void) {
    if (g_listen_fd >= 0) return;
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    if (fd < 0) return;

    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    int n = snprintf(addr.sun_path + 1, sizeof(addr.sun_path) - 2,
                     "clipsd-vk-%u-%u", (unsigned)getuid(), (unsigned)getpid());
    socklen_t len = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + 1 + n);
    if (bind(fd, (struct sockaddr *)&addr, len) != 0 || listen(fd, 2) != 0) {
        close(fd);
        return;
    }
    g_listen_fd = fd;
}

static void disconnect_client_locked(void) {
    if (g_client_fd >= 0) {
        close(g_client_fd);
        g_client_fd = -1;
    }
    g_setup_sent_for_active = false;
    g_active_swap = VK_NULL_HANDLE;
}

static void poll_accept_locked(void) {
    if (g_client_fd >= 0 || g_listen_fd < 0) return;
    int cfd = accept4(g_listen_fd, NULL, NULL, SOCK_CLOEXEC);
    if (cfd < 0) return;

    struct ucred cred;
    socklen_t clen = sizeof(cred);
    if (getsockopt(cfd, SOL_SOCKET, SO_PEERCRED, &cred, &clen) != 0 || cred.uid != getuid()) {
        close(cfd);
        return;
    }

    struct timeval tv = { .tv_sec = 0, .tv_usec = 200000 };
    setsockopt(cfd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    setsockopt(cfd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
    ClvkHello hello;
    ssize_t r = recv(cfd, &hello, sizeof(hello), MSG_WAITALL);
    if (r != (ssize_t)sizeof(hello) || hello.magic != CLVK_MAGIC) {
        close(cfd);
        return;
    }
    g_target_fps = (hello.target_fps >= 1 && hello.target_fps <= 240) ? hello.target_fps : 60;
    g_client_fd = cfd;
    g_setup_sent_for_active = false;
}

static bool send_msg_with_fds(int fd, const ClvkMsg *msg, const int *fds, int nfds) {
    struct iovec iov = { .iov_base = (void *)msg, .iov_len = sizeof(*msg) };
    struct msghdr hdr;
    memset(&hdr, 0, sizeof(hdr));
    hdr.msg_iov = &iov;
    hdr.msg_iovlen = 1;

    char cbuf[CMSG_SPACE(sizeof(int) * NUM_SLOTS)];
    if (nfds > 0 && fds) {
        memset(cbuf, 0, sizeof(cbuf));
        hdr.msg_control = cbuf;
        hdr.msg_controllen = CMSG_SPACE(sizeof(int) * nfds);
        struct cmsghdr *cm = CMSG_FIRSTHDR(&hdr);
        cm->cmsg_level = SOL_SOCKET;
        cm->cmsg_type = SCM_RIGHTS;
        cm->cmsg_len = CMSG_LEN(sizeof(int) * nfds);
        memcpy(CMSG_DATA(cm), fds, sizeof(int) * nfds);
    }
    ssize_t sent = sendmsg(fd, &hdr, MSG_NOSIGNAL);
    return sent == (ssize_t)sizeof(*msg);
}

static void *notify_thread_main(void *arg) {
    SwapchainData *sc = (SwapchainData *)arg;
    DevData *dev = sc->dev;
    for (;;) {
        pthread_mutex_lock(&sc->notify_mu);
        while (!sc->notify_stop && sc->notify_head == sc->notify_tail) {
            pthread_cond_wait(&sc->notify_cv, &sc->notify_mu);
        }
        if (sc->notify_stop) {
            pthread_mutex_unlock(&sc->notify_mu);
            break;
        }
        PendingNotify item = sc->notify_q[sc->notify_head];
        sc->notify_head = (sc->notify_head + 1) & 3;
        pthread_mutex_unlock(&sc->notify_mu);

        VkFence fence = sc->slots[item.slot].fence;
        if (dev->WaitForFences(dev->device, 1, &fence, 1, 100000000ULL) != VK_SUCCESS) {
            continue;
        }

        ClvkMsg msg;
        memset(&msg, 0, sizeof(msg));
        msg.magic = CLVK_MAGIC;
        msg.kind = CLVK_MSG_FRAME;
        msg.width = sc->width;
        msg.height = sc->height;
        msg.fourcc = DRM_FORMAT_ARGB8888;
        msg.slot = item.slot;
        msg.modifier = sc->modifier;
        msg.pts_us = item.pts_us;

        pthread_mutex_lock(&g_mu);
        int cfd = g_client_fd;
        bool ok = (cfd >= 0) && send_msg_with_fds(cfd, &msg, NULL, 0);
        if (!ok && cfd >= 0) {
            disconnect_client_locked();
        }
        pthread_mutex_unlock(&g_mu);
    }
    return NULL;
}

static void destroy_capture_resources(SwapchainData *sc) {
    if (!sc->capture_ready) return;
    DevData *dev = sc->dev;

    if (sc->notify_running) {
        pthread_mutex_lock(&sc->notify_mu);
        sc->notify_stop = true;
        pthread_cond_broadcast(&sc->notify_cv);
        pthread_mutex_unlock(&sc->notify_mu);
        pthread_join(sc->notify_thread, NULL);
        sc->notify_running = false;
    }

    if (dev && dev->DeviceWaitIdle) {
        dev->DeviceWaitIdle(dev->device);
    }
    for (int i = 0; i < NUM_SLOTS; i++) {
        CaptureSlot *s = &sc->slots[i];
        if (s->dmabuf_fd >= 0) { close(s->dmabuf_fd); s->dmabuf_fd = -1; }
        if (s->fence) { dev->DestroyFence(dev->device, s->fence, NULL); s->fence = 0; }
        if (s->sem) { dev->DestroySemaphore(dev->device, s->sem, NULL); s->sem = 0; }
        if (s->image) { dev->DestroyImage(dev->device, s->image, NULL); s->image = 0; }
        if (s->mem) { dev->FreeMemory(dev->device, s->mem, NULL); s->mem = 0; }
    }
    if (sc->cmd_pool) {
        dev->DestroyCommandPool(dev->device, sc->cmd_pool, NULL);
        sc->cmd_pool = 0;
    }
    sc->capture_ready = false;
}

static int find_memory_type(DevData *dev, uint32_t type_bits, VkFlags req_flags) {
    VkPhysicalDeviceMemoryProperties mp;
    memset(&mp, 0, sizeof(mp));
    dev->inst->GetPhysicalDeviceMemoryProperties(dev->phys, &mp);
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++) {
        if ((type_bits & (1U << i)) && (mp.memoryTypes[i].propertyFlags & req_flags) == req_flags) {
            return (int)i;
        }
    }
    return -1;
}

static bool init_capture_resources(SwapchainData *sc, uint32_t queue_family) {
    DevData *dev = sc->dev;
    if (!dev || !dev->has_dmabuf_ext || !dev->GetMemoryFdKHR) return false;
    if (sc->width == 0 || sc->height == 0) return false;

    for (int i = 0; i < NUM_SLOTS; i++) {
        memset(&sc->slots[i], 0, sizeof(CaptureSlot));
        sc->slots[i].dmabuf_fd = -1;
    }

    uint64_t chosen_mods[64];
    uint32_t chosen_mod_count = 0;
    bool use_modifier_tiling = false;

    if (dev->has_modifier_ext && dev->inst->GetPhysicalDeviceFormatProperties2 &&
        dev->GetImageDrmFormatModifierPropertiesEXT) {
        VkDrmFormatModifierPropertiesListEXT mod_list;
        memset(&mod_list, 0, sizeof(mod_list));
        mod_list.sType = VK_STRUCTURE_TYPE_DRM_FORMAT_MODIFIER_PROPERTIES_LIST_EXT;
        VkFormatProperties2 fp2;
        memset(&fp2, 0, sizeof(fp2));
        fp2.sType = VK_STRUCTURE_TYPE_FORMAT_PROPERTIES_2;
        fp2.pNext = &mod_list;
        dev->inst->GetPhysicalDeviceFormatProperties2(dev->phys, VK_FORMAT_B8G8R8A8_UNORM, &fp2);

        if (mod_list.drmFormatModifierCount > 0) {
            uint32_t cnt = mod_list.drmFormatModifierCount;
            if (cnt > 64) cnt = 64;
            VkDrmFormatModifierPropertiesEXT props[64];
            mod_list.drmFormatModifierCount = cnt;
            mod_list.pDrmFormatModifierProperties = props;
            dev->inst->GetPhysicalDeviceFormatProperties2(dev->phys, VK_FORMAT_B8G8R8A8_UNORM, &fp2);

            /* Prefer non-zero (tiled NVIDIA/Mesa) modifiers so EGL imports as GL_TEXTURE_2D
             * (on NVIDIA, linear mod=0 is external_only=true). */
            for (uint32_t i = 0; i < cnt && chosen_mod_count < 64; i++) {
                if (props[i].drmFormatModifierPlaneCount == 1 &&
                    (props[i].drmFormatModifierTilingFeatures & VK_FORMAT_FEATURE_TRANSFER_DST_BIT) &&
                    props[i].drmFormatModifier != 0) {
                    chosen_mods[chosen_mod_count++] = props[i].drmFormatModifier;
                }
            }
            if (chosen_mod_count == 0) {
                for (uint32_t i = 0; i < cnt && chosen_mod_count < 64; i++) {
                    if (props[i].drmFormatModifierPlaneCount == 1 &&
                        (props[i].drmFormatModifierTilingFeatures & VK_FORMAT_FEATURE_TRANSFER_DST_BIT)) {
                        chosen_mods[chosen_mod_count++] = props[i].drmFormatModifier;
                    }
                }
            }
            if (chosen_mod_count > 0) use_modifier_tiling = true;
        }
    }

    VkCommandPoolCreateInfo cpci = {
        .sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
        .pNext = NULL,
        .flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
        .queueFamilyIndex = queue_family,
    };
    if (dev->CreateCommandPool(dev->device, &cpci, NULL, &sc->cmd_pool) != VK_SUCCESS) {
        return false;
    }

    VkCommandBuffer cmds[NUM_SLOTS];
    VkCommandBufferAllocateInfo cbai = {
        .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
        .pNext = NULL,
        .commandPool = sc->cmd_pool,
        .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
        .commandBufferCount = NUM_SLOTS,
    };
    if (dev->AllocateCommandBuffers(dev->device, &cbai, cmds) != VK_SUCCESS) {
        goto fail;
    }
    for (int i = 0; i < NUM_SLOTS; i++) {
        /* Copy device dispatch table pointer into layer-allocated command buffers! */
        *(void **)cmds[i] = *(void **)dev->device;
        sc->slots[i].cmd = cmds[i];
    }

    sc->modifier = DRM_FORMAT_MOD_INVALID;
    for (int i = 0; i < NUM_SLOTS; i++) {
        CaptureSlot *s = &sc->slots[i];

        VkImageDrmFormatModifierListCreateInfoEXT mod_ci = {
            .sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_LIST_CREATE_INFO_EXT,
            .pNext = NULL,
            .drmFormatModifierCount = chosen_mod_count,
            .pDrmFormatModifiers = chosen_mods,
        };
        VkExternalMemoryImageCreateInfo ext_ci = {
            .sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
            .pNext = use_modifier_tiling ? (const void *)&mod_ci : NULL,
            .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
        };
        VkImageCreateInfo ici = {
            .sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
            .pNext = &ext_ci,
            .flags = 0,
            .imageType = VK_IMAGE_TYPE_2D,
            .format = VK_FORMAT_B8G8R8A8_UNORM,
            .extent = { sc->width, sc->height, 1 },
            .mipLevels = 1,
            .arrayLayers = 1,
            .samples = VK_SAMPLE_COUNT_1_BIT,
            .tiling = use_modifier_tiling ? VK_IMAGE_TILING_DRM_FORMAT_MODIFIER_EXT : VK_IMAGE_TILING_OPTIMAL,
            .usage = VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_SAMPLED_BIT,
            .sharingMode = VK_SHARING_MODE_EXCLUSIVE,
            .queueFamilyIndexCount = 0,
            .pQueueFamilyIndices = NULL,
            .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
        };
        if (dev->CreateImage(dev->device, &ici, NULL, &s->image) != VK_SUCCESS) {
            goto fail;
        }

        VkMemoryRequirements mr;
        memset(&mr, 0, sizeof(mr));
        dev->GetImageMemoryRequirements(dev->device, s->image, &mr);
        int mem_idx = find_memory_type(dev, mr.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
        if (mem_idx < 0) mem_idx = find_memory_type(dev, mr.memoryTypeBits, 0);
        if (mem_idx < 0) goto fail;

        VkMemoryDedicatedAllocateInfo ded_ai = {
            .sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
            .pNext = NULL,
            .image = s->image,
            .buffer = 0,
        };
        VkExportMemoryAllocateInfo exp_ai = {
            .sType = VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO,
            .pNext = &ded_ai,
            .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
        };
        VkMemoryAllocateInfo mai = {
            .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
            .pNext = &exp_ai,
            .allocationSize = mr.size,
            .memoryTypeIndex = (uint32_t)mem_idx,
        };
        if (dev->AllocateMemory(dev->device, &mai, NULL, &s->mem) != VK_SUCCESS) {
            goto fail;
        }
        if (dev->BindImageMemory(dev->device, s->image, s->mem, 0) != VK_SUCCESS) {
            goto fail;
        }

        VkMemoryGetFdInfoKHR gfi = {
            .sType = VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR,
            .pNext = NULL,
            .memory = s->mem,
            .handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
        };
        if (dev->GetMemoryFdKHR(dev->device, &gfi, &s->dmabuf_fd) != VK_SUCCESS || s->dmabuf_fd < 0) {
            goto fail;
        }

        if (use_modifier_tiling) {
            VkImageDrmFormatModifierPropertiesEXT mp = {
                .sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_PROPERTIES_EXT,
                .pNext = NULL,
                .drmFormatModifier = DRM_FORMAT_MOD_INVALID,
            };
            if (dev->GetImageDrmFormatModifierPropertiesEXT(dev->device, s->image, &mp) == VK_SUCCESS) {
                sc->modifier = mp.drmFormatModifier;
            }
            VkImageSubresource sub = { VK_IMAGE_ASPECT_MEMORY_PLANE_0_BIT, 0, 0 };
            VkSubresourceLayout lay;
            memset(&lay, 0, sizeof(lay));
            dev->GetImageSubresourceLayout(dev->device, s->image, &sub, &lay);
            s->stride = (uint32_t)(lay.rowPitch > 0 ? lay.rowPitch : sc->width * 4);
            s->offset = (uint32_t)lay.offset;
        } else {
            s->stride = sc->width * 4;
            s->offset = 0;
        }

        VkSemaphoreCreateInfo sci = { VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO, NULL, 0 };
        VkFenceCreateInfo fci = { VK_STRUCTURE_TYPE_FENCE_CREATE_INFO, NULL, VK_FENCE_CREATE_SIGNALED_BIT };
        if (dev->CreateSemaphore(dev->device, &sci, NULL, &s->sem) != VK_SUCCESS ||
            dev->CreateFence(dev->device, &fci, NULL, &s->fence) != VK_SUCCESS) {
            goto fail;
        }
    }

    sc->queue_family = queue_family;
    sc->next_slot = 0;
    sc->next_due_us = 0;
    sc->notify_stop = false;
    sc->notify_head = 0;
    sc->notify_tail = 0;
    pthread_mutex_init(&sc->notify_mu, NULL);
    pthread_cond_init(&sc->notify_cv, NULL);
    if (pthread_create(&sc->notify_thread, NULL, notify_thread_main, sc) != 0) {
        goto fail;
    }
    sc->notify_running = true;
    sc->capture_ready = true;
    return true;

fail:
    sc->capture_ready = true;
    destroy_capture_resources(sc);
    return false;
}

static bool send_setup_locked(SwapchainData *sc) {
    if (g_client_fd < 0 || !sc->capture_ready) return false;
    ClvkMsg msg;
    memset(&msg, 0, sizeof(msg));
    msg.magic = CLVK_MAGIC;
    msg.kind = CLVK_MSG_SETUP;
    msg.width = sc->width;
    msg.height = sc->height;
    msg.fourcc = DRM_FORMAT_ARGB8888;
    msg.slot = NUM_SLOTS;
    msg.modifier = sc->modifier;
    int fds[NUM_SLOTS];
    for (int i = 0; i < NUM_SLOTS; i++) {
        msg.stride[i] = sc->slots[i].stride;
        msg.offset[i] = sc->slots[i].offset;
        fds[i] = sc->slots[i].dmabuf_fd;
    }
    if (!send_msg_with_fds(g_client_fd, &msg, fds, NUM_SLOTS)) {
        disconnect_client_locked();
        return false;
    }
    g_setup_sent_for_active = true;
    g_active_swap = sc->swapchain;
    return true;
}

static VkResult clipsd_CreateInstance(
    const VkInstanceCreateInfo *pCreateInfo,
    const void *pAllocator,
    VkInstance *pInstance
) {
    VkLayerInstanceCreateInfo *link = (VkLayerInstanceCreateInfo *)pCreateInfo->pNext;
    while (link && !(link->sType == VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO && link->function == 0)) {
        link = (VkLayerInstanceCreateInfo *)link->pNext;
    }
    if (!link || !link->u.pLayerInfo) return -1;

    PFN_vkGetInstanceProcAddr next_gipa = link->u.pLayerInfo->pfnNextGetInstanceProcAddr;
    link->u.pLayerInfo = link->u.pLayerInfo->pNext;

    PFN_vkCreateInstance next_create = (PFN_vkCreateInstance)next_gipa(NULL, "vkCreateInstance");
    if (!next_create) return -1;

    VkResult res = next_create(pCreateInfo, pAllocator, pInstance);
    if (res != VK_SUCCESS) return res;

    pthread_mutex_lock(&g_mu);
    for (int i = 0; i < MAX_INSTANCES; i++) {
        if (g_insts[i].key == NULL) {
            InstData *id = &g_insts[i];
            memset(id, 0, sizeof(*id));
            id->key = dispatch_key(*pInstance);
            id->instance = *pInstance;
            id->next_gipa = next_gipa;
            id->DestroyInstance = (PFN_vkDestroyInstance)next_gipa(*pInstance, "vkDestroyInstance");
            id->EnumerateDeviceExtensionProperties = (PFN_vkEnumerateDeviceExtensionProperties)next_gipa(*pInstance, "vkEnumerateDeviceExtensionProperties");
            id->GetPhysicalDeviceMemoryProperties = (PFN_vkGetPhysicalDeviceMemoryProperties)next_gipa(*pInstance, "vkGetPhysicalDeviceMemoryProperties");
            id->GetPhysicalDeviceFormatProperties2 = (PFN_vkGetPhysicalDeviceFormatProperties2)next_gipa(*pInstance, "vkGetPhysicalDeviceFormatProperties2");
            if (!id->GetPhysicalDeviceFormatProperties2) {
                id->GetPhysicalDeviceFormatProperties2 = (PFN_vkGetPhysicalDeviceFormatProperties2)next_gipa(*pInstance, "vkGetPhysicalDeviceFormatProperties2KHR");
            }
            id->GetPhysicalDeviceSurfaceCapabilitiesKHR = (PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR)next_gipa(*pInstance, "vkGetPhysicalDeviceSurfaceCapabilitiesKHR");
            break;
        }
    }
    pthread_mutex_unlock(&g_mu);
    return res;
}

static void clipsd_DestroyInstance(VkInstance instance, const void *pAllocator) {
    if (!instance) return;
    PFN_vkDestroyInstance next = NULL;
    pthread_mutex_lock(&g_mu);
    InstData *id = find_inst(instance);
    if (id) {
        next = id->DestroyInstance;
        memset(id, 0, sizeof(*id));
    }
    pthread_mutex_unlock(&g_mu);
    if (next) next(instance, pAllocator);
}

static bool has_ext_in_list(const char *const *list, uint32_t count, const char *name) {
    for (uint32_t i = 0; i < count; i++) {
        if (strcmp(list[i], name) == 0) return true;
    }
    return false;
}

static bool phys_supports_ext(const VkExtensionProperties *avail, uint32_t count, const char *name) {
    for (uint32_t i = 0; i < count; i++) {
        if (strcmp(avail[i].extensionName, name) == 0) return true;
    }
    return false;
}

static VkResult clipsd_CreateDevice(
    VkPhysicalDevice phys,
    const VkDeviceCreateInfo *pCreateInfo,
    const void *pAllocator,
    VkDevice *pDevice
) {
    VkLayerDeviceCreateInfo *link = (VkLayerDeviceCreateInfo *)pCreateInfo->pNext;
    while (link && !(link->sType == VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO && link->function == 0)) {
        link = (VkLayerDeviceCreateInfo *)link->pNext;
    }
    if (!link || !link->u.pLayerInfo) return -1;

    PFN_vkGetInstanceProcAddr next_gipa = link->u.pLayerInfo->pfnNextGetInstanceProcAddr;
    PFN_vkGetDeviceProcAddr next_gdpa = link->u.pLayerInfo->pfnNextGetDeviceProcAddr;
    link->u.pLayerInfo = link->u.pLayerInfo->pNext;

    pthread_mutex_lock(&g_mu);
    InstData *id = find_inst(phys);
    pthread_mutex_unlock(&g_mu);

    static const char *const kWantExts[] = {
        "VK_KHR_external_memory",
        "VK_KHR_external_memory_fd",
        "VK_EXT_external_memory_dma_buf",
        "VK_EXT_image_drm_format_modifier",
        "VK_KHR_image_format_list",
    };
    const uint32_t n_want = sizeof(kWantExts) / sizeof(kWantExts[0]);

    VkDeviceCreateInfo mod_ci = *pCreateInfo;
    const char **exts = NULL;
    bool has_dmabuf_ext = false;
    bool has_modifier_ext = false;

    if (id && id->EnumerateDeviceExtensionProperties) {
        uint32_t avail_n = 0;
        id->EnumerateDeviceExtensionProperties(phys, NULL, &avail_n, NULL);
        VkExtensionProperties *avail = avail_n > 0 ? malloc(sizeof(VkExtensionProperties) * avail_n) : NULL;
        if (avail && id->EnumerateDeviceExtensionProperties(phys, NULL, &avail_n, avail) == VK_SUCCESS) {
            exts = malloc(sizeof(const char *) * (pCreateInfo->enabledExtensionCount + n_want));
            if (exts) {
                uint32_t out_n = pCreateInfo->enabledExtensionCount;
                for (uint32_t i = 0; i < out_n; i++) exts[i] = pCreateInfo->ppEnabledExtensionNames[i];
                for (uint32_t i = 0; i < n_want; i++) {
                    if (phys_supports_ext(avail, avail_n, kWantExts[i]) &&
                        !has_ext_in_list(exts, out_n, kWantExts[i])) {
                        exts[out_n++] = kWantExts[i];
                    }
                }
                has_dmabuf_ext = has_ext_in_list(exts, out_n, "VK_KHR_external_memory_fd") &&
                                 has_ext_in_list(exts, out_n, "VK_EXT_external_memory_dma_buf");
                has_modifier_ext = has_ext_in_list(exts, out_n, "VK_EXT_image_drm_format_modifier");
                mod_ci.enabledExtensionCount = out_n;
                mod_ci.ppEnabledExtensionNames = exts;
            }
        }
        free(avail);
    }

    PFN_vkCreateDevice next_create = (PFN_vkCreateDevice)next_gipa(id ? id->instance : NULL, "vkCreateDevice");
    if (!next_create) { free(exts); return -1; }

    VkResult res = next_create(phys, &mod_ci, pAllocator, pDevice);
    free(exts);
    if (res != VK_SUCCESS) return res;

    pthread_mutex_lock(&g_mu);
    for (int i = 0; i < MAX_DEVICES; i++) {
        if (g_devs[i].key == NULL) {
            DevData *d = &g_devs[i];
            memset(d, 0, sizeof(*d));
            d->key = dispatch_key(*pDevice);
            d->device = *pDevice;
            d->phys = phys;
            d->inst = id;
            d->has_dmabuf_ext = has_dmabuf_ext;
            d->has_modifier_ext = has_modifier_ext;
            d->next_gdpa = next_gdpa;
            #define LOAD(fn) d->fn = (PFN_vk##fn)next_gdpa(*pDevice, "vk" #fn)
            LOAD(DestroyDevice);
            LOAD(GetDeviceQueue);
            LOAD(GetDeviceQueue2);
            LOAD(CreateSwapchainKHR);
            LOAD(DestroySwapchainKHR);
            LOAD(GetSwapchainImagesKHR);
            LOAD(QueuePresentKHR);
            LOAD(CreateImage);
            LOAD(DestroyImage);
            LOAD(GetImageMemoryRequirements);
            LOAD(AllocateMemory);
            LOAD(FreeMemory);
            LOAD(BindImageMemory);
            LOAD(GetMemoryFdKHR);
            LOAD(GetImageDrmFormatModifierPropertiesEXT);
            LOAD(GetImageSubresourceLayout);
            LOAD(CreateCommandPool);
            LOAD(DestroyCommandPool);
            LOAD(AllocateCommandBuffers);
            LOAD(BeginCommandBuffer);
            LOAD(EndCommandBuffer);
            LOAD(ResetCommandBuffer);
            LOAD(CmdPipelineBarrier);
            LOAD(CmdBlitImage);
            LOAD(QueueSubmit);
            LOAD(CreateSemaphore);
            LOAD(DestroySemaphore);
            LOAD(CreateFence);
            LOAD(DestroyFence);
            LOAD(GetFenceStatus);
            LOAD(WaitForFences);
            LOAD(ResetFences);
            LOAD(DeviceWaitIdle);
            #undef LOAD
            break;
        }
    }
    pthread_mutex_unlock(&g_mu);
    return res;
}

static void clipsd_DestroyDevice(VkDevice device, const void *pAllocator) {
    if (!device) return;
    PFN_vkDestroyDevice next = NULL;
    pthread_mutex_lock(&g_mu);
    DevData *d = find_dev(device);
    if (d) {
        for (int i = 0; i < MAX_SWAPCHAINS; i++) {
            if (g_swaps[i].dev == d && g_swaps[i].swapchain) {
                destroy_capture_resources(&g_swaps[i]);
                free(g_swaps[i].images);
                memset(&g_swaps[i], 0, sizeof(SwapchainData));
            }
        }
        next = d->DestroyDevice;
        memset(d, 0, sizeof(*d));
    }
    pthread_mutex_unlock(&g_mu);
    if (next) next(device, pAllocator);
}

static void clipsd_GetDeviceQueue(VkDevice device, uint32_t family, uint32_t idx, VkQueue *pQueue) {
    pthread_mutex_lock(&g_mu);
    DevData *d = find_dev(device);
    PFN_vkGetDeviceQueue next = d ? d->GetDeviceQueue : NULL;
    pthread_mutex_unlock(&g_mu);
    if (next) next(device, family, idx, pQueue);
    if (pQueue && *pQueue) {
        pthread_mutex_lock(&g_mu);
        record_queue_family(*pQueue, family);
        pthread_mutex_unlock(&g_mu);
    }
}

static void clipsd_GetDeviceQueue2(VkDevice device, const VkDeviceQueueInfo2 *info, VkQueue *pQueue) {
    pthread_mutex_lock(&g_mu);
    DevData *d = find_dev(device);
    PFN_vkGetDeviceQueue2 next = d ? d->GetDeviceQueue2 : NULL;
    pthread_mutex_unlock(&g_mu);
    if (next) next(device, info, pQueue);
    if (info && pQueue && *pQueue) {
        pthread_mutex_lock(&g_mu);
        record_queue_family(*pQueue, info->queueFamilyIndex);
        pthread_mutex_unlock(&g_mu);
    }
}

static VkResult clipsd_CreateSwapchainKHR(
    VkDevice device,
    const VkSwapchainCreateInfoKHR *pCreateInfo,
    const void *pAllocator,
    VkSwapchainKHR *pSwapchain
) {
    pthread_mutex_lock(&g_mu);
    DevData *d = find_dev(device);
    pthread_mutex_unlock(&g_mu);
    if (!d || !d->CreateSwapchainKHR) return -1;

    VkSwapchainCreateInfoKHR mod_ci = *pCreateInfo;
    if (d->inst && d->inst->GetPhysicalDeviceSurfaceCapabilitiesKHR) {
        VkSurfaceCapabilitiesKHR caps;
        memset(&caps, 0, sizeof(caps));
        if (d->inst->GetPhysicalDeviceSurfaceCapabilitiesKHR(d->phys, pCreateInfo->surface, &caps) == VK_SUCCESS) {
            if (caps.supportedUsageFlags & VK_IMAGE_USAGE_TRANSFER_SRC_BIT) {
                mod_ci.imageUsage |= VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
            }
        }
    } else {
        mod_ci.imageUsage |= VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
    }

    VkResult res = d->CreateSwapchainKHR(device, &mod_ci, pAllocator, pSwapchain);
    if (res != VK_SUCCESS) {
        /* Fallback to unmodified usage flags if driver rejected TRANSFER_SRC. */
        res = d->CreateSwapchainKHR(device, pCreateInfo, pAllocator, pSwapchain);
        if (res != VK_SUCCESS) return res;
    }

    uint32_t count = 0;
    d->GetSwapchainImagesKHR(device, *pSwapchain, &count, NULL);
    VkImage *imgs = count > 0 ? malloc(sizeof(VkImage) * count) : NULL;
    if (imgs && d->GetSwapchainImagesKHR(device, *pSwapchain, &count, imgs) != VK_SUCCESS) {
        free(imgs);
        imgs = NULL;
        count = 0;
    }

    pthread_mutex_lock(&g_mu);
    ensure_listener_locked();
    for (int i = 0; i < MAX_SWAPCHAINS; i++) {
        if (g_swaps[i].swapchain == VK_NULL_HANDLE) {
            SwapchainData *sc = &g_swaps[i];
            memset(sc, 0, sizeof(*sc));
            sc->swapchain = *pSwapchain;
            sc->dev = d;
            sc->width = pCreateInfo->imageExtent.width;
            sc->height = pCreateInfo->imageExtent.height;
            sc->format = pCreateInfo->imageFormat;
            sc->image_count = count;
            sc->images = imgs;
            break;
        }
    }
    pthread_mutex_unlock(&g_mu);
    return res;
}

static void clipsd_DestroySwapchainKHR(VkDevice device, VkSwapchainKHR swapchain, const void *pAllocator) {
    pthread_mutex_lock(&g_mu);
    DevData *d = find_dev(device);
    SwapchainData *sc = find_swap(swapchain);
    if (sc) {
        if (g_active_swap == swapchain) {
            g_setup_sent_for_active = false;
            g_active_swap = VK_NULL_HANDLE;
        }
        destroy_capture_resources(sc);
        free(sc->images);
        memset(sc, 0, sizeof(*sc));
    }
    PFN_vkDestroySwapchainKHR next = d ? d->DestroySwapchainKHR : NULL;
    pthread_mutex_unlock(&g_mu);
    if (next) next(device, swapchain, pAllocator);
}

static VkResult clipsd_QueuePresentKHR(VkQueue queue, const VkPresentInfoKHR *pPresentInfo) {
    pthread_mutex_lock(&g_mu);
    DevData *d = find_dev(queue);
    if (!d || !d->QueuePresentKHR) {
        pthread_mutex_unlock(&g_mu);
        return -1;
    }
    SwapchainData *sc = (pPresentInfo && pPresentInfo->swapchainCount > 0)
                            ? find_swap(pPresentInfo->pSwapchains[0])
                            : NULL;

    /* Dormant fast path: poll accept4() only once every 15 presents (~4 Hz at 60 fps). */
    if (g_client_fd < 0) {
        if (sc && (++sc->accept_poll_div % 15U == 1U)) {
            poll_accept_locked();
        }
        if (g_client_fd < 0) {
            if (sc && sc->capture_ready) {
                destroy_capture_resources(sc);
            }
            PFN_vkQueuePresentKHR next = d->QueuePresentKHR;
            pthread_mutex_unlock(&g_mu);
            return next(queue, pPresentInfo);
        }
    }

    if (!sc || !sc->images || pPresentInfo->pImageIndices[0] >= sc->image_count) {
        PFN_vkQueuePresentKHR next = d->QueuePresentKHR;
        pthread_mutex_unlock(&g_mu);
        return next(queue, pPresentInfo);
    }

    uint32_t qfam = find_queue_family(queue);
    if (!sc->capture_ready) {
        if (!init_capture_resources(sc, qfam)) {
            disconnect_client_locked();
            PFN_vkQueuePresentKHR next = d->QueuePresentKHR;
            pthread_mutex_unlock(&g_mu);
            return next(queue, pPresentInfo);
        }
    }
    if (!g_setup_sent_for_active || g_active_swap != sc->swapchain) {
        if (!send_setup_locked(sc)) {
            PFN_vkQueuePresentKHR next = d->QueuePresentKHR;
            pthread_mutex_unlock(&g_mu);
            return next(queue, pPresentInfo);
        }
    }

    int64_t now = monotonic_us();
    int64_t frame_us = 1000000LL / (int64_t)(g_target_fps > 0 ? g_target_fps : 60);
    int64_t tol = frame_us / 8;
    if (now + tol < sc->next_due_us) {
        PFN_vkQueuePresentKHR next = d->QueuePresentKHR;
        pthread_mutex_unlock(&g_mu);
        return next(queue, pPresentInfo);
    }

    uint32_t slot_idx = sc->next_slot & 1U;
    CaptureSlot *slot = &sc->slots[slot_idx];
    if (d->GetFenceStatus(d->device, slot->fence) != VK_SUCCESS) {
        /* Previous blit on this slot still running; skip so we never stall the game. */
        PFN_vkQueuePresentKHR next = d->QueuePresentKHR;
        pthread_mutex_unlock(&g_mu);
        return next(queue, pPresentInfo);
    }

    sc->next_due_us = (sc->next_due_us == 0 || now > sc->next_due_us + 2 * frame_us)
                          ? (now + frame_us)
                          : (sc->next_due_us + frame_us);
    sc->next_slot++;

    VkImage src_img = sc->images[pPresentInfo->pImageIndices[0]];
    d->ResetFences(d->device, 1, &slot->fence);
    d->ResetCommandBuffer(slot->cmd, 0);

    VkCommandBufferBeginInfo bi = {
        .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
        .pNext = NULL,
        .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
        .pInheritanceInfo = NULL,
    };
    d->BeginCommandBuffer(slot->cmd, &bi);

    VkImageMemoryBarrier pre[2] = {
        {
            .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
            .pNext = NULL,
            .srcAccessMask = 0,
            .dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
            .oldLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
            .newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
            .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
            .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
            .image = src_img,
            .subresourceRange = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 },
        },
        {
            .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
            .pNext = NULL,
            .srcAccessMask = 0,
            .dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
            .oldLayout = VK_IMAGE_LAYOUT_UNDEFINED,
            .newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
            .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
            .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
            .image = slot->image,
            .subresourceRange = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 },
        },
    };
    d->CmdPipelineBarrier(slot->cmd, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
                          0, 0, NULL, 0, NULL, 2, pre);

    VkImageBlit region = {
        .srcSubresource = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1 },
        .srcOffsets = { { 0, 0, 0 }, { (int32_t)sc->width, (int32_t)sc->height, 1 } },
        .dstSubresource = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1 },
        .dstOffsets = { { 0, 0, 0 }, { (int32_t)sc->width, (int32_t)sc->height, 1 } },
    };
    d->CmdBlitImage(slot->cmd, src_img, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                    slot->image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region, VK_FILTER_NEAREST);

    VkImageMemoryBarrier post[2] = {
        {
            .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
            .pNext = NULL,
            .srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
            .dstAccessMask = 0,
            .oldLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
            .newLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
            .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
            .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
            .image = src_img,
            .subresourceRange = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 },
        },
        {
            .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
            .pNext = NULL,
            .srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
            .dstAccessMask = 0,
            .oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
            .newLayout = VK_IMAGE_LAYOUT_GENERAL,
            .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
            .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
            .image = slot->image,
            .subresourceRange = { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 },
        },
    };
    d->CmdPipelineBarrier(slot->cmd, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
                          0, 0, NULL, 0, NULL, 2, post);
    d->EndCommandBuffer(slot->cmd);

    VkFlags stages_stack[16];
    uint32_t wcnt = pPresentInfo->waitSemaphoreCount;
    VkFlags *stages = wcnt <= 16 ? stages_stack : malloc(sizeof(VkFlags) * wcnt);
    for (uint32_t i = 0; i < wcnt; i++) stages[i] = VK_PIPELINE_STAGE_TRANSFER_BIT;

    VkSubmitInfo si = {
        .sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
        .pNext = NULL,
        .waitSemaphoreCount = wcnt,
        .pWaitSemaphores = pPresentInfo->pWaitSemaphores,
        .pWaitDstStageMask = stages,
        .commandBufferCount = 1,
        .pCommandBuffers = &slot->cmd,
        .signalSemaphoreCount = 1,
        .pSignalSemaphores = &slot->sem,
    };
    VkResult sub_res = d->QueueSubmit(queue, 1, &si, slot->fence);
    if (stages != stages_stack) free(stages);

    VkPresentInfoKHR mod_pi = *pPresentInfo;
    if (sub_res == VK_SUCCESS) {
        mod_pi.waitSemaphoreCount = 1;
        mod_pi.pWaitSemaphores = &slot->sem;

        /* Enqueue async fence-wait + socket notify so vkQueuePresentKHR never blocks. */
        pthread_mutex_lock(&sc->notify_mu);
        int next_tail = (sc->notify_tail + 1) & 3;
        if (next_tail != sc->notify_head) {
            sc->notify_q[sc->notify_tail] = (PendingNotify){ .in_use = true, .slot = slot_idx, .pts_us = now };
            sc->notify_tail = next_tail;
            pthread_cond_signal(&sc->notify_cv);
        }
        pthread_mutex_unlock(&sc->notify_mu);
    }

    PFN_vkQueuePresentKHR next = d->QueuePresentKHR;
    pthread_mutex_unlock(&g_mu);
    return next(queue, &mod_pi);
}

__attribute__((visibility("default")))
PFN_vkVoidFunction clipsd_GetDeviceProcAddr(VkDevice device, const char *pName) {
    if (!pName) return NULL;
    if (strcmp(pName, "vkGetDeviceProcAddr") == 0)  return (PFN_vkVoidFunction)clipsd_GetDeviceProcAddr;
    if (strcmp(pName, "vkDestroyDevice") == 0)      return (PFN_vkVoidFunction)clipsd_DestroyDevice;
    if (strcmp(pName, "vkGetDeviceQueue") == 0)     return (PFN_vkVoidFunction)clipsd_GetDeviceQueue;
    if (strcmp(pName, "vkGetDeviceQueue2") == 0)    return (PFN_vkVoidFunction)clipsd_GetDeviceQueue2;
    if (strcmp(pName, "vkCreateSwapchainKHR") == 0) return (PFN_vkVoidFunction)clipsd_CreateSwapchainKHR;
    if (strcmp(pName, "vkDestroySwapchainKHR") == 0)return (PFN_vkVoidFunction)clipsd_DestroySwapchainKHR;
    if (strcmp(pName, "vkQueuePresentKHR") == 0)    return (PFN_vkVoidFunction)clipsd_QueuePresentKHR;

    if (!device) return NULL;
    pthread_mutex_lock(&g_mu);
    DevData *d = find_dev(device);
    PFN_vkGetDeviceProcAddr next = d ? d->next_gdpa : NULL;
    pthread_mutex_unlock(&g_mu);
    return next ? next(device, pName) : NULL;
}

__attribute__((visibility("default")))
PFN_vkVoidFunction clipsd_GetInstanceProcAddr(VkInstance instance, const char *pName) {
    if (!pName) return NULL;
    if (strcmp(pName, "vkGetInstanceProcAddr") == 0) return (PFN_vkVoidFunction)clipsd_GetInstanceProcAddr;
    if (strcmp(pName, "vkCreateInstance") == 0)      return (PFN_vkVoidFunction)clipsd_CreateInstance;
    if (strcmp(pName, "vkDestroyInstance") == 0)     return (PFN_vkVoidFunction)clipsd_DestroyInstance;
    if (strcmp(pName, "vkCreateDevice") == 0)        return (PFN_vkVoidFunction)clipsd_CreateDevice;
    if (strcmp(pName, "vkGetDeviceProcAddr") == 0)   return (PFN_vkVoidFunction)clipsd_GetDeviceProcAddr;

    if (!instance) return NULL;
    pthread_mutex_lock(&g_mu);
    InstData *id = find_inst(instance);
    PFN_vkGetInstanceProcAddr next = id ? id->next_gipa : NULL;
    pthread_mutex_unlock(&g_mu);
    return next ? next(instance, pName) : NULL;
}

__attribute__((visibility("default")))
VkResult vkNegotiateLoaderLayerInterfaceVersion(VkNegotiateLayerInterface *pVersionStruct) {
    if (!pVersionStruct || pVersionStruct->sType != 1) return -1;
    if (pVersionStruct->loaderLayerInterfaceVersion > 2) {
        pVersionStruct->loaderLayerInterfaceVersion = 2;
    }
    pVersionStruct->pfnGetInstanceProcAddr = clipsd_GetInstanceProcAddr;
    pVersionStruct->pfnGetDeviceProcAddr = clipsd_GetDeviceProcAddr;
    pVersionStruct->pfnGetPhysicalDeviceProcAddr = NULL;
    return VK_SUCCESS;
}
