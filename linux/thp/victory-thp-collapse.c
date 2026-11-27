/* Collapses 2 MiB-aligned pieces of another process's memory into transparent
 * huge pages (process_madvise MADV_COLLAPSE). Needs CAP_SYS_NICE, given once:
 *   sudo setcap cap_sys_nice+ep victory-thp-collapse
 * (linux/victory-proton.sh runs it by itself every 15 s; see linux/thp/README.md.)
 * Usage: victory-thp-collapse <pid> <start-hex> <end-hex> [<start> <end>...]
 * Prints: pieces collapsed, pieces not (first error). Touches nothing else. */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <unistd.h>

#ifndef MADV_COLLAPSE
#define MADV_COLLAPSE 25
#endif
#define HUGE (2UL << 20)

int main(int argc, char **argv) {
    if (argc < 4 || (argc - 2) % 2) {
        fprintf(stderr, "usage: %s pid start end [start end...]\n", argv[0]);
        return 2;
    }
    int pid = atoi(argv[1]);
    int fd = (int)syscall(SYS_pidfd_open, pid, 0);
    if (fd < 0) {
        fprintf(stderr, "pidfd_open: %s\n", strerror(errno));
        return 1;
    }
    unsigned long ok = 0, bad = 0;
    int first_err = 0;
    for (int i = 2; i + 1 < argc; i += 2) {
        unsigned long a = strtoul(argv[i], NULL, 16), b = strtoul(argv[i + 1], NULL, 16);
        for (unsigned long at = (a + HUGE - 1) & ~(HUGE - 1); at + HUGE <= b; at += HUGE) {
            struct iovec v = {(void *)at, HUGE};
            if (syscall(SYS_process_madvise, fd, &v, 1, MADV_COLLAPSE, 0) < 0) {
                if (!first_err) first_err = errno;
                bad++;
            } else {
                ok++;
            }
        }
    }
    printf("%lu collapsed, %lu not%s%s\n", ok, bad, bad ? ": " : "", bad ? strerror(first_err) : "");
    return 0;
}
