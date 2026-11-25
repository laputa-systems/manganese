#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "zlib.h"
#include "cpu_features.h"

static uint32_t get32(const unsigned char *p) {
    return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | p[3];
}

static void put32(unsigned char *p, uint32_t n) {
    p[0] = n >> 24; p[1] = n >> 16; p[2] = n >> 8; p[3] = n;
}

static int record(const unsigned char *in, uint32_t n, unsigned char **result, uint32_t *size) {
    if (n < 7) return -1;
    unsigned op = in[0], chunks = ((unsigned)in[5] << 8) | in[6];
    int level = (int8_t)in[1], window = (int8_t)in[2], mem = in[3], strategy = in[4];
    size_t pos = 7, used = 4, cap = 65540;
    unsigned char *out = malloc(cap);
    if (!out) return -1;
    z_stream stream = {0};
    int status = Z_OK;
    uLong sum = op == 2 ? crc32(0, Z_NULL, 0) : adler32(0, Z_NULL, 0);
    if (op == 0) status = deflateInit2(&stream, level, Z_DEFLATED, window, mem, strategy);
    else if (op == 1) status = inflateInit2(&stream, window);
    else if (op > 3) { free(out); return -1; }
    int initialized = status == Z_OK;
    for (unsigned i = 0; i < chunks; i++) {
        if (pos + 5 > n) { free(out); return -1; }
        uint32_t len = get32(in + pos); int flush = in[pos + 4]; pos += 5;
        if (len > n - pos) { free(out); return -1; }
        if (op >= 2) sum = op == 2 ? crc32(sum, in + pos, len) : adler32(sum, in + pos, len);
        else if (initialized && status >= 0 && status != Z_STREAM_END) {
            stream.next_in = (Bytef *)(in + pos); stream.avail_in = len;
            do {
                if (cap - used < 65536) {
                    if (cap > UINT32_MAX / 2) { free(out); return -1; }
                    cap *= 2; unsigned char *next = realloc(out, cap);
                    if (!next) { free(out); return -1; } out = next;
                }
                stream.next_out = out + used; stream.avail_out = 65536;
                status = op == 0 ? deflate(&stream, flush) : inflate(&stream, flush);
                used += 65536 - stream.avail_out;
                if (status < 0 || status == Z_STREAM_END) break;
            } while (stream.avail_in || stream.avail_out == 0);
        }
        pos += len;
    }
    if (pos != n) { free(out); return -1; }
    if (initialized && op == 0) deflateEnd(&stream);
    if (initialized && op == 1) inflateEnd(&stream);
    put32(out, (uint32_t)status);
    if (op >= 2) { put32(out + 4, (uint32_t)sum); used = 8; }
    *result = out; *size = (uint32_t)used; return 0;
}

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--cpu") == 0) {
        cpu_check_features();
        printf("sse2=%d ssse3=%d simd_sse42_pclmul=%d avx512=%d\n", x86_cpu_enable_sse2, x86_cpu_enable_ssse3, x86_cpu_enable_simd, x86_cpu_enable_avx512);
        return 0;
    }
    unsigned char head[4];
    if (fread(head, 1, 4, stdin) != 4 || memcmp(head, "MGB1", 4)) return 1;
    if (fwrite("MGB1", 1, 4, stdout) != 4 || fflush(stdout)) return 1;
    while (fread(head, 1, 4, stdin) == 4) {
        uint32_t n = get32(head), size;
        unsigned char *in = malloc(n ? n : 1), *out;
        if (!in || fread(in, 1, n, stdin) != n || record(in, n, &out, &size)) return 1;
        free(in); put32(head, size);
        if (fwrite(head, 1, 4, stdout) != 4 || fwrite(out, 1, size, stdout) != size || fflush(stdout)) return 1;
        free(out);
    }
    return ferror(stdin) ? 1 : 0;
}
