// Proof of concept: kernel write tracking on mapped section views under a patched Wine.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <memoryapi.h>
#include <stdio.h>
#include <string.h>
#define PAGE64 0x10000
#define SPAN 0x80000000u
static double now_us(void){ LARGE_INTEGER c,f; QueryPerformanceCounter(&c); QueryPerformanceFrequency(&f); return (double)c.QuadPart*1e6/(double)f.QuadPart; }
static ULONG_PTR ask(void *base, SIZE_T size, DWORD flags, void **out, ULONG_PTR cap){
    ULONG_PTR n = cap; DWORD gran = 0;
    UINT r = GetWriteWatch(flags, base, size, out, &n, &gran);
    if (r) return (ULONG_PTR)-1;
    return n;
}
int main(void){
    static void *pages[65536];
    SIZE_T secsize = 256u << 20;
    HANDLE sec = CreateFileMappingW(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE | SEC_COMMIT, 0, (DWORD)secsize, NULL);
    if (!sec) { printf("no section %lu\n", GetLastError()); return 1; }
    // 1. one plain view with the flag
    char *a = MapViewOfFile3(sec, NULL, NULL, 0, 16 * PAGE64, MEM_WRITE_WATCH, PAGE_READWRITE, NULL, 0);
    printf("map with MEM_WRITE_WATCH: %p (error %lu)\n", a, a ? 0 : GetLastError());
    if (!a) return 2;
    ULONG_PTR n = ask(a, 16 * PAGE64, WRITE_WATCH_FLAG_RESET, pages, 65536);
    printf("first asking after mapping: %lld written (-1: the call failed, error %lu)\n", (long long)n, GetLastError());
    if (n == (ULONG_PTR)-1) { printf("RESULT: write watch on mapped views is NOT supported here\n"); return 3; }
    n = ask(a, 16 * PAGE64, WRITE_WATCH_FLAG_RESET, pages, 65536);
    printf("second asking, nothing written: %lld\n", (long long)n);
    a[0x1000] = 1; a[0x5008] = 2; a[15 * PAGE64 + 0xfff0] = 3;
    n = ask(a, 16 * PAGE64, WRITE_WATCH_FLAG_RESET, pages, 65536);
    printf("after 3 writes: %lld written:", (long long)n);
    for (ULONG_PTR i = 0; i < n && i < 8; i++) printf(" +0x%llx", (unsigned long long)((char *)pages[i] - a));
    printf("\n");
    n = ask(a, 16 * PAGE64, WRITE_WATCH_FLAG_RESET, pages, 65536);
    printf("asked again after the reset: %lld\n", (long long)n);
    // 2. a second view of the same pages: a write through one, asked at the other
    char *b = MapViewOfFile3(sec, NULL, NULL, 0, 16 * PAGE64, MEM_WRITE_WATCH, PAGE_READWRITE, NULL, 0);
    ask(b, 16 * PAGE64, WRITE_WATCH_FLAG_RESET, pages, 65536);
    a[0x2000] = 7;
    printf("alias: written through A, read at B = %d; asked at B: %lld, asked at A: %lld\n", b[0x2000],
           (long long)ask(b, 16 * PAGE64, WRITE_WATCH_FLAG_RESET, pages, 65536), (long long)ask(a, 16 * PAGE64, WRITE_WATCH_FLAG_RESET, pages, 65536));
    // 3. many views of one 64 KiB page each in a placeholder, scattered offsets, as the title maps them
    enum { VIEWS = 64 };
    char *hold = VirtualAlloc2(NULL, NULL, VIEWS * PAGE64, MEM_RESERVE | MEM_RESERVE_PLACEHOLDER, PAGE_NOACCESS, NULL, 0);
    printf("placeholder: %p\n", hold);
    for (int i = 0; i < VIEWS - 1; i++) VirtualFree(hold + i * PAGE64, PAGE64, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER);
    int ok = 0;
    for (int i = 0; i < VIEWS; i++) {
        ULONG64 off = (ULONG64)((i * 37) % 1024 + 64) * PAGE64;
        if (MapViewOfFile3(sec, NULL, hold + i * PAGE64, off, PAGE64, MEM_REPLACE_PLACEHOLDER | MEM_WRITE_WATCH, PAGE_READWRITE, NULL, 0)) ok++;
    }
    printf("views in the placeholder: %d of %d\n", ok, VIEWS);
    n = ask(hold, VIEWS * PAGE64, WRITE_WATCH_FLAG_RESET, pages, 65536);
    printf("plain asking over all of them (several views): %lld (-1: refused)\n", (long long)n);
    n = ask(hold, VIEWS * PAGE64, WRITE_WATCH_FLAG_RESET | SPAN, pages, 65536);
    printf("span asking over all of them, first: %lld\n", (long long)n);
    n = ask(hold, VIEWS * PAGE64, WRITE_WATCH_FLAG_RESET | SPAN, pages, 65536);
    printf("span asking again, nothing written: %lld\n", (long long)n);
    for (int i = 0; i < VIEWS; i += 4) hold[i * PAGE64 + 0x3000] = (char)i;
    n = ask(hold, VIEWS * PAGE64, WRITE_WATCH_FLAG_RESET | SPAN, pages, 65536);
    printf("span asking after %d writes in different views: %lld written, first at +0x%llx\n", VIEWS / 4, (long long)n, n && n != (ULONG_PTR)-1 ? (unsigned long long)((char *)pages[0] - hold) : 0ull);
    // 4. costs
    double t0 = now_us(); int rounds = 2000;
    for (int r = 0; r < rounds; r++) { hold[(r % VIEWS) * PAGE64 + 0x100] = (char)r; ask(hold, VIEWS * PAGE64, WRITE_WATCH_FLAG_RESET | SPAN, pages, 65536); }
    double t1 = now_us();
    printf("cost: a span asking over %d views (4 MiB) with one page written: %.1f us\n", VIEWS, (t1 - t0) / rounds);
    t0 = now_us();
    for (int r = 0; r < rounds; r++) { ask(hold + (r % VIEWS) * PAGE64, PAGE64, WRITE_WATCH_FLAG_RESET, pages, 65536); }
    t1 = now_us();
    printf("cost: an asking of one 64 KiB view: %.1f us\n", (t1 - t0) / rounds);
    for (int pass = 0; pass < 2; pass++) {
        DWORD flag = pass ? MEM_WRITE_WATCH : 0;
        t0 = now_us();
        for (int r = 0; r < rounds; r++) {
            char *at = hold + (r % VIEWS) * PAGE64;
            UnmapViewOfFile2(GetCurrentProcess(), at, MEM_PRESERVE_PLACEHOLDER);
            MapViewOfFile3(sec, NULL, at, (ULONG64)(r % 512 + 2048) * PAGE64, PAGE64, MEM_REPLACE_PLACEHOLDER | flag, PAGE_READWRITE, NULL, 0);
            at[0] = 1;
        }
        t1 = now_us();
        printf("cost: unmap + map + first write of a 64 KiB view %s the flag: %.1f us\n", pass ? "with" : "without", (t1 - t0) / rounds);
    }
    // 5. a fault-free write: time 100000 first writes after resets
    t0 = now_us();
    for (int r = 0; r < 200; r++) { for (int i = 0; i < VIEWS; i++) hold[i * PAGE64 + 0x4000] = (char)r; ask(hold, VIEWS * PAGE64, WRITE_WATCH_FLAG_RESET | SPAN, pages, 65536); }
    t1 = now_us();
    printf("cost: %d first-writes-after-reset plus one span asking: %.1f us (%.2f us a write)\n", VIEWS, (t1 - t0) / 200, (t1 - t0) / 200 / VIEWS);
    printf("RESULT: write watch on mapped views works\n");
    return 0;
}
