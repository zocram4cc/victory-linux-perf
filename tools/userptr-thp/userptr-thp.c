/* LD_PRELOAD shim: memory handed to the Radeon driver as a userptr is made
 * 2 MiB transparent huge pages, from inside the process (no capability
 * needed for one's own memory).
 *
 * amdgpu walks every page of a userptr object on every submission
 * (hmm_range_fault). The title's 32-bit graphics heap opened by renderer port
 * 1 is ~185 MB: ~47,000 entries as 4 KiB pages, ~93 as huge pages (found by
 * kroeeee-eeee, VIctory-Linux-Proton-Wine-7900XT-XTX 47fd862, who collapse it
 * from outside with CAP_SYS_NICE). Here: madvise(MADV_COLLAPSE) on the whole
 * 2 MiB pieces of the range when the object is made, and again every 15 s
 * (pieces split by protection changes are put together again).
 *
 * USERPTR_THP=0 turns it off; USERPTR_THP_LOG=<file> appends what it did.
 * Build: gcc -O2 -shared -fPIC -o libuserptr-thp.so userptr-thp.c -lpthread -ldl */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#ifndef MADV_COLLAPSE
#define MADV_COLLAPSE 25
#endif
#define HUGE (2UL << 20)
#define MOST 64

struct amdgpu_device;
typedef int (*create_fn)(struct amdgpu_device *, void *, uint64_t, void *);

static struct { uintptr_t at, end; } ranges[MOST];
static int count;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_once_t once = PTHREAD_ONCE_INIT;

static void say(const char *what, uintptr_t at, uintptr_t end, unsigned long ok, unsigned long bad, int err) {
    const char *path = getenv("USERPTR_THP_LOG");
    if (!path) return;
    FILE *f = fopen(path, "a");
    if (!f) return;
    fprintf(f, "%ld %d %s %#lx-%#lx: %lu collapsed, %lu not%s%s\n", (long)time(NULL), (int)getpid(), what,
            (unsigned long)at, (unsigned long)end, ok, bad, bad ? ": " : "", bad ? strerror(err) : "");
    fclose(f);
}

static void collapse(const char *what, uintptr_t at, uintptr_t end, int quiet) {
    unsigned long ok = 0, bad = 0;
    int err = 0;
    for (uintptr_t p = (at + HUGE - 1) & ~(HUGE - 1); p + HUGE <= end; p += HUGE) {
        if (madvise((void *)p, HUGE, MADV_COLLAPSE) < 0) {
            if (!err) err = errno;
            bad++;
        } else {
            ok++;
        }
    }
    if (!quiet || bad) say(what, at, end, ok, bad, err);
}

static void *again(void *unused) {
    (void)unused;
    static const unsigned waits[] = {1, 2, 4, 8};
    for (unsigned round = 0;; round++) {
        sleep(round < 4 ? waits[round] : 15);
        pthread_mutex_lock(&lock);
        int n = count;
        pthread_mutex_unlock(&lock);
        for (int i = 0; i < n; i++) collapse("again", ranges[i].at, ranges[i].end, round > 5);
    }
    return NULL;
}

static void start(void) {
    pthread_t t;
    if (!pthread_create(&t, NULL, again, NULL)) pthread_detach(t);
}

int amdgpu_create_bo_from_user_mem(struct amdgpu_device *dev, void *cpu, uint64_t size, void *bo) {
    static create_fn real;
    if (!real) real = (create_fn)dlsym(RTLD_NEXT, "amdgpu_create_bo_from_user_mem");
    if (!real) {
        /* The Vulkan driver is loaded with dlopen, so its libdrm_amdgpu is
         * not in the global order RTLD_NEXT walks: ask the library itself
         * (the same file the loader gave the driver). Returning an error
         * here would make vkd3d-proton open an empty heap without a word:
         * a black picture at a very good frame rate. */
        void *lib = dlopen("libdrm_amdgpu.so.1", RTLD_NOW | RTLD_LOCAL);
        if (lib) real = (create_fn)dlsym(lib, "amdgpu_create_bo_from_user_mem");
    }
    if (!real) {
        say("no real amdgpu_create_bo_from_user_mem", (uintptr_t)cpu, (uintptr_t)cpu + size, 0, 1, ENOSYS);
        return -ENOSYS;
    }
    const char *off = getenv("USERPTR_THP");
    /* Only big objects: small imports gain nothing and are many. */
    if (!(off && off[0] == '0') && size >= 16 * HUGE) {
        uintptr_t at = (uintptr_t)cpu, end = at + size;
        madvise((void *)(at & ~4095UL), size, MADV_HUGEPAGE);
        collapse("made", at, end, 0);
        pthread_mutex_lock(&lock);
        if (count < MOST) {
            ranges[count].at = at;
            ranges[count].end = end;
            count++;
        }
        pthread_mutex_unlock(&lock);
        pthread_once(&once, start);
    }
    return real(dev, cpu, size, bo);
}
