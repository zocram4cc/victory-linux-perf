// M.2 step 1, the Linux side: VRAM mapped for the CPU at an address we
// choose (VK_EXT_map_memory_placed), and the same VRAM mapped again at a
// second address through a dma-buf (the page pool's aliases). Checks that
// a CPU write through one address reads back through the other, and that
// the GPU sees it (a copy into a host-visible buffer read by the CPU).
//
//   cc -O2 -o placed-alias placed-alias.c -lvulkan && ./placed-alias
#define VK_ENABLE_BETA_EXTENSIONS
#include <vulkan/vulkan.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <sys/mman.h>
#include <unistd.h>

#define CHECK(x) do { VkResult r_ = (x); if (r_ != VK_SUCCESS) { fprintf(stderr, "%s: %d\n", #x, r_); exit(1); } } while (0)

int main(void) {
    const VkDeviceSize size = 64 << 20;
    VkApplicationInfo app = {VK_STRUCTURE_TYPE_APPLICATION_INFO};
    app.apiVersion = VK_API_VERSION_1_3;
    VkInstanceCreateInfo ii = {VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO};
    ii.pApplicationInfo = &app;
    VkInstance inst;
    CHECK(vkCreateInstance(&ii, NULL, &inst));
    uint32_t n = 8;
    VkPhysicalDevice devs[8], phys = VK_NULL_HANDLE;
    CHECK(vkEnumeratePhysicalDevices(inst, &n, devs));
    for (uint32_t i = 0; i < n; i++) {
        VkPhysicalDeviceProperties p;
        vkGetPhysicalDeviceProperties(devs[i], &p);
        if (p.deviceType == VK_PHYSICAL_DEVICE_TYPE_DISCRETE_GPU) { phys = devs[i]; printf("%s\n", p.deviceName); break; }
    }
    VkPhysicalDeviceMapMemoryPlacedFeaturesEXT placed_f = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_MAP_MEMORY_PLACED_FEATURES_EXT};
    VkPhysicalDeviceFeatures2 f2 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2, &placed_f};
    vkGetPhysicalDeviceFeatures2(phys, &f2);
    printf("memoryMapPlaced %u, memoryMapRangePlaced %u, memoryUnmapReserve %u\n",
           placed_f.memoryMapPlaced, placed_f.memoryMapRangePlaced, placed_f.memoryUnmapReserve);
    VkPhysicalDeviceMapMemoryPlacedPropertiesEXT placed_p = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_MAP_MEMORY_PLACED_PROPERTIES_EXT};
    VkPhysicalDeviceProperties2 p2 = {VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2, &placed_p};
    vkGetPhysicalDeviceProperties2(phys, &p2);
    printf("minPlacedMemoryMapAlignment %llu\n", (unsigned long long)placed_p.minPlacedMemoryMapAlignment);

    uint32_t family = 0, qn = 16;
    VkQueueFamilyProperties qf[16];
    vkGetPhysicalDeviceQueueFamilyProperties(phys, &qn, qf);
    for (uint32_t i = 0; i < qn; i++) if (qf[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) { family = i; break; }
    float prio = 1;
    VkDeviceQueueCreateInfo qi = {VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO, NULL, 0, family, 1, &prio};
    const char *exts[] = {"VK_EXT_map_memory_placed", "VK_KHR_map_memory2", "VK_KHR_external_memory_fd", "VK_EXT_external_memory_dma_buf"};
    VkDeviceCreateInfo di = {VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO, &placed_f};
    di.queueCreateInfoCount = 1;
    di.pQueueCreateInfos = &qi;
    di.enabledExtensionCount = 4;
    di.ppEnabledExtensionNames = exts;
    VkDevice dev;
    CHECK(vkCreateDevice(phys, &di, NULL, &dev));
    VkQueue queue;
    vkGetDeviceQueue(dev, family, 0, &queue);

    VkPhysicalDeviceMemoryProperties mem;
    vkGetPhysicalDeviceMemoryProperties(phys, &mem);
    uint32_t rebar = UINT32_MAX, readback = UINT32_MAX;
    for (uint32_t t = 0; t < mem.memoryTypeCount; t++) {
        VkMemoryPropertyFlags fl = mem.memoryTypes[t].propertyFlags;
        if (rebar == UINT32_MAX && (fl & 7) == 7 && !(fl & 0xc0)) rebar = t;
        if (readback == UINT32_MAX && (fl & VK_MEMORY_PROPERTY_HOST_CACHED_BIT) && !(fl & 1)) readback = t;
    }
    printf("ReBAR type %u, read-back type %u\n", rebar, readback);

    // VRAM, exportable as a dma-buf.
    VkExportMemoryAllocateInfo export = {VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO, NULL, VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT};
    VkMemoryAllocateInfo ai = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, &export, size, rebar};
    VkDeviceMemory vram;
    CHECK(vkAllocateMemory(dev, &ai, NULL, &vram));

    // 1. Placed map at an address we reserved (as Wine reserves the title's ranges).
    void *want = mmap((void *)0x7e0000000000ull, size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE | MAP_FIXED_NOREPLACE, -1, 0);
    printf("reserved %p\n", want);
    VkMemoryMapPlacedInfoEXT pi = {VK_STRUCTURE_TYPE_MEMORY_MAP_PLACED_INFO_EXT, NULL, want};
    VkMemoryMapInfoKHR mi = {VK_STRUCTURE_TYPE_MEMORY_MAP_INFO_KHR, &pi, VK_MEMORY_MAP_PLACED_BIT_EXT, vram, 0, VK_WHOLE_SIZE};
    PFN_vkMapMemory2KHR map2 = (PFN_vkMapMemory2KHR)vkGetDeviceProcAddr(dev, "vkMapMemory2KHR");
    void *a = NULL;
    VkResult r = map2(dev, &mi, &a);
    printf("placed map: result %d at %p (asked %p)\n", r, a, want);
    if (r != VK_SUCCESS || a != want) return 1;

    // 2. The same VRAM again, through a dma-buf, at a second address.
    VkMemoryGetFdInfoKHR gi = {VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR, NULL, vram, VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT};
    PFN_vkGetMemoryFdKHR getfd = (PFN_vkGetMemoryFdKHR)vkGetDeviceProcAddr(dev, "vkGetMemoryFdKHR");
    int fd = -1;
    CHECK(getfd(dev, &gi, &fd));
    void *b = mmap((void *)0x7f0000000000ull, size, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED_NOREPLACE, fd, 0);
    printf("dma-buf alias: %p (%s)\n", b, b == MAP_FAILED ? "failed" : "ok");
    // A 64 KiB page of it at a third address, at an offset (a scattered alias).
    void *c = mmap((void *)0x7f8000000000ull, 64 << 10, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED_NOREPLACE, fd, 5 << 20);
    printf("one 64 KiB page of it at an offset: %p (%s)\n", c, c == MAP_FAILED ? "failed" : "ok");

    // 3. Writes through one address, reads through the others.
    memset(a, 0, size);
    ((volatile uint32_t *)a)[0] = 0x1234abcd;
    ((volatile uint32_t *)((char *)a + (5 << 20)))[0] = 0x5555aaaa;
    if (b != MAP_FAILED) ((volatile uint32_t *)b)[1] = 0x0badf00d;
    if (b != MAP_FAILED)
        printf("through the alias: %08x (want 1234abcd); through the placed map: %08x (want 0badf00d)\n",
               ((volatile uint32_t *)b)[0], ((volatile uint32_t *)a)[1]);
    if (c != MAP_FAILED) printf("through the page alias: %08x (want 5555aaaa)\n", ((volatile uint32_t *)c)[0]);

    // 4. The GPU sees the CPU's writes: copy the first 64 bytes into cached memory.
    VkBufferCreateInfo bi = {VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO};
    bi.size = size;
    bi.usage = VK_BUFFER_USAGE_TRANSFER_SRC_BIT;
    VkExternalMemoryBufferCreateInfo eb = {VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_BUFFER_CREATE_INFO, NULL, VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT};
    bi.pNext = &eb;
    VkBuffer src, dst;
    CHECK(vkCreateBuffer(dev, &bi, NULL, &src));
    CHECK(vkBindBufferMemory(dev, src, vram, 0));
    VkBufferCreateInfo bd = {VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO};
    bd.size = 4096;
    bd.usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT;
    CHECK(vkCreateBuffer(dev, &bd, NULL, &dst));
    VkMemoryRequirements req;
    vkGetBufferMemoryRequirements(dev, dst, &req);
    VkMemoryAllocateInfo ad = {VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, NULL, req.size, readback};
    VkDeviceMemory dmem;
    CHECK(vkAllocateMemory(dev, &ad, NULL, &dmem));
    CHECK(vkBindBufferMemory(dev, dst, dmem, 0));
    VkCommandPoolCreateInfo cpi = {VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO, NULL, 0, family};
    VkCommandPool pool;
    CHECK(vkCreateCommandPool(dev, &cpi, NULL, &pool));
    VkCommandBufferAllocateInfo cai = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO, NULL, pool, VK_COMMAND_BUFFER_LEVEL_PRIMARY, 1};
    VkCommandBuffer cb;
    CHECK(vkAllocateCommandBuffers(dev, &cai, &cb));
    VkCommandBufferBeginInfo cbi = {VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
    CHECK(vkBeginCommandBuffer(cb, &cbi));
    VkBufferCopy regions[2] = {{0, 0, 16}, {5 << 20, 16, 16}};
    vkCmdCopyBuffer(cb, src, dst, 2, regions);
    CHECK(vkEndCommandBuffer(cb));
    VkSubmitInfo si = {VK_STRUCTURE_TYPE_SUBMIT_INFO};
    si.commandBufferCount = 1;
    si.pCommandBuffers = &cb;
    CHECK(vkQueueSubmit(queue, 1, &si, VK_NULL_HANDLE));
    CHECK(vkQueueWaitIdle(queue));
    uint32_t *out;
    CHECK(vkMapMemory(dev, dmem, 0, VK_WHOLE_SIZE, 0, (void **)&out));
    printf("the GPU read: %08x %08x, and at 5 MiB %08x (want 1234abcd 0badf00d, 5555aaaa)\n", out[0], out[1], out[4]);
    return 0;
}
