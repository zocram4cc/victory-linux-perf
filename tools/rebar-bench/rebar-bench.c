// Phase 2.1 microbenchmark: how fast the CPU writes and reads memory of
// each Vulkan memory type (VRAM through ReBAR, system memory), and how
// fast the GPU copies out of it. Native Linux, no game, no Wine:
//
//   cc -O2 -o rebar-bench rebar-bench.c -lvulkan && ./rebar-bench [MB]
//
// Prints, per host-visible memory type of the first discrete GPU: CPU
// sequential write (memset and 32-byte stores), CPU sequential read,
// CPU read-modify-write of scattered 64-byte lines, and a GPU copy of
// the buffer into plain VRAM.
#include <vulkan/vulkan.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <time.h>
#include <immintrin.h>

#define CHECK(x) do { VkResult r_ = (x); if (r_ != VK_SUCCESS) { fprintf(stderr, "%s: %d\n", #x, r_); exit(1); } } while (0)

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec * 1e-9;
}

static VkDevice dev;
static VkPhysicalDevice phys;
static VkPhysicalDeviceMemoryProperties mem;
static VkQueue queue;
static uint32_t family;

static VkBuffer make_buffer(VkDeviceSize size, VkDeviceMemory *out, uint32_t type) {
    VkBufferCreateInfo bi = {VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO};
    bi.size = size;
    bi.usage = VK_BUFFER_USAGE_TRANSFER_SRC_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT;
    VkBuffer b;
    CHECK(vkCreateBuffer(dev, &bi, NULL, &b));
    VkMemoryRequirements req;
    vkGetBufferMemoryRequirements(dev, b, &req);
    if (!(req.memoryTypeBits & (1u << type))) { vkDestroyBuffer(dev, b, NULL); return VK_NULL_HANDLE; }
    VkMemoryAllocateInfo ai = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO};
    ai.allocationSize = req.size;
    ai.memoryTypeIndex = type;
    if (vkAllocateMemory(dev, &ai, NULL, out) != VK_SUCCESS) { vkDestroyBuffer(dev, b, NULL); return VK_NULL_HANDLE; }
    CHECK(vkBindBufferMemory(dev, b, *out, 0));
    return b;
}

static double gpu_copy(VkBuffer src, VkBuffer dst, VkDeviceSize size) {
    VkCommandPoolCreateInfo pi = {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO};
    pi.queueFamilyIndex = family;
    VkCommandPool pool;
    CHECK(vkCreateCommandPool(dev, &pi, NULL, &pool));
    VkCommandBufferAllocateInfo ai = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO};
    ai.commandPool = pool;
    ai.commandBufferCount = 1;
    VkCommandBuffer cb;
    CHECK(vkAllocateCommandBuffers(dev, &ai, &cb));
    VkCommandBufferBeginInfo bi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
    CHECK(vkBeginCommandBuffer(cb, &bi));
    VkBufferCopy c = {0, 0, size};
    for (int i = 0; i < 4; i++) vkCmdCopyBuffer(cb, src, dst, 1, &c);
    CHECK(vkEndCommandBuffer(cb));
    VkFenceCreateInfo fi = {VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    VkFence f;
    CHECK(vkCreateFence(dev, &fi, NULL, &f));
    VkSubmitInfo si = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
    si.commandBufferCount = 1;
    si.pCommandBuffers = &cb;
    double t = now();
    CHECK(vkQueueSubmit(queue, 1, &si, f));
    CHECK(vkWaitForFences(dev, 1, &f, VK_TRUE, UINT64_MAX));
    t = now() - t;
    vkDestroyFence(dev, f, NULL);
    vkDestroyCommandPool(dev, pool, NULL);
    return 4.0 * size / t / 1e9;
}

int main(int argc, char **argv) {
    size_t mb = argc > 1 ? (size_t)atoi(argv[1]) : 256;
    size_t size = mb << 20;
    VkApplicationInfo app = {VK_STRUCTURE_TYPE_APPLICATION_INFO};
    app.apiVersion = VK_API_VERSION_1_1;
    VkInstanceCreateInfo ii = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
    ii.pApplicationInfo = &app;
    VkInstance inst;
    CHECK(vkCreateInstance(&ii, NULL, &inst));
    uint32_t n = 8;
    VkPhysicalDevice devs[8];
    CHECK(vkEnumeratePhysicalDevices(inst, &n, devs));
    for (uint32_t i = 0; i < n; i++) {
        VkPhysicalDeviceProperties p;
        vkGetPhysicalDeviceProperties(devs[i], &p);
        if (p.deviceType == VK_PHYSICAL_DEVICE_TYPE_DISCRETE_GPU) { phys = devs[i]; printf("%s\n", p.deviceName); break; }
    }
    if (!phys) { fprintf(stderr, "no discrete GPU\n"); return 1; }
    vkGetPhysicalDeviceMemoryProperties(phys, &mem);
    uint32_t qn = 16;
    VkQueueFamilyProperties qf[16];
    vkGetPhysicalDeviceQueueFamilyProperties(phys, &qn, qf);
    for (uint32_t i = 0; i < qn; i++) if (qf[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) { family = i; break; }
    float prio = 1.0f;
    VkDeviceQueueCreateInfo qi = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO};
    qi.queueFamilyIndex = family;
    qi.queueCount = 1;
    qi.pQueuePriorities = &prio;
    VkDeviceCreateInfo di = {VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO};
    di.queueCreateInfoCount = 1;
    di.pQueueCreateInfos = &qi;
    CHECK(vkCreateDevice(phys, &di, NULL, &dev));
    vkGetDeviceQueue(dev, family, 0, &queue);

    // Plain VRAM destination for the GPU copies.
    uint32_t vram = UINT32_MAX;
    for (uint32_t t = 0; t < mem.memoryTypeCount; t++)
        if (mem.memoryTypes[t].propertyFlags == VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) { vram = t; break; }
    VkDeviceMemory dst_mem;
    VkBuffer dst = make_buffer(size, &dst_mem, vram);

    char *local = aligned_alloc(4096, size);
    memset(local, 1, size);
    printf("%zu MB per test\n", mb);
    printf("%-34s %10s %10s %10s %12s %10s\n", "memory type", "CPU memset", "CPU 32B st", "CPU read", "CPU rmw 64B", "GPU copy");
    uint32_t done_flags[64];
    int ndone = 0;
    for (uint32_t t = 0; t < mem.memoryTypeCount; t++) {
        VkMemoryPropertyFlags f = mem.memoryTypes[t].propertyFlags;
        if (!(f & VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT)) continue;
        int seen = 0;
        for (int k = 0; k < ndone; k++) if (done_flags[k] == f) seen = 1;
        if (seen) continue;
        done_flags[ndone++] = f;
        VkDeviceMemory m;
        VkBuffer b = make_buffer(size, &m, t);
        if (!b) continue;
        void *p;
        CHECK(vkMapMemory(dev, m, 0, VK_WHOLE_SIZE, 0, &p));
        char name[64];
        snprintf(name, sizeof name, "%u %s%s%s%s", t,
                 f & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT ? "VRAM " : "RAM ",
                 f & VK_MEMORY_PROPERTY_HOST_COHERENT_BIT ? "coherent " : "",
                 f & VK_MEMORY_PROPERTY_HOST_CACHED_BIT ? "cached " : "",
                 f & 0x40 ? "devcoh" : "");
        double t0 = now();
        memset(p, 2, size);
        double w1 = size / (now() - t0) / 1e9;
        t0 = now();
        __m256i v = _mm256_set1_epi32(3);
        for (size_t i = 0; i < size; i += 32) _mm256_stream_si256((__m256i *)((char *)p + i), v);
        _mm_sfence();
        double w2 = size / (now() - t0) / 1e9;
        // Reads are slow on uncached memory: a sixteenth of the size.
        size_t rsize = size / 16;
        t0 = now();
        memcpy(local, p, rsize);
        double r = rsize / (now() - t0) / 1e9;
        // Scattered read-modify-write of 64-byte lines (what a CPU does to
        // structures it updates in place).
        size_t lines = rsize / 64 / 16;
        uint64_t x = 88172645463325252ull;
        t0 = now();
        for (size_t i = 0; i < lines; i++) {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            volatile uint64_t *q = (uint64_t *)((char *)p + (x % (size / 64)) * 64);
            *q = *q + 1;
        }
        double rmw = lines / (now() - t0) / 1e6;
        double g = dst ? gpu_copy(b, dst, size) : 0;
        printf("%-34s %8.2f GB/s %6.2f GB/s %6.3f GB/s %8.2f M/s %6.1f GB/s\n", name, w1, w2, r, rmw, g);
        vkUnmapMemory(dev, m);
        vkDestroyBuffer(dev, b, NULL);
        vkFreeMemory(dev, m, NULL);
    }
    // Reference: plain RAM for the CPU.
    double t0 = now();
    memset(local, 4, size);
    printf("%-34s %8.2f GB/s\n", "malloc (CPU reference)", size / (now() - t0) / 1e9);
    return 0;
}
