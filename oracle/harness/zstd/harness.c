#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define ZSTD_STATIC_LINKING_ONLY
#include "third_party/zstd/src/lib/zstd.h"
#include "third_party/zstd/src/lib/zstd_errors.h"

static uint32_t get32(const unsigned char *p) {
    return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | p[3];
}

static void put32(unsigned char *p, uint32_t n) {
    p[0] = n >> 24; p[1] = n >> 16; p[2] = n >> 8; p[3] = n;
}

static int append(unsigned char **out, size_t *used, size_t *cap, const void *data, size_t len) {
    if (len > UINT32_MAX - *used) return -1;
    if (*used + len > *cap) {
        size_t nextcap = (*used + len) * 2;
        unsigned char *next = realloc(*out, nextcap);
        if (!next) return -1;
        *out = next; *cap = nextcap;
    }
    memcpy(*out + *used, data, len); *used += len;
    return 0;
}

static int record(const unsigned char *in, uint32_t n, unsigned char **result, uint32_t *size) {
    if (n < 12) return -1;
    uint32_t output_size = get32(in), dict_size = get32(in + 4), chunks = get32(in + 8);
    if (!output_size || output_size > 1048576 || dict_size > n - 12) return -1;
    size_t pos = 12 + dict_size, used = 4, cap = 64;
    unsigned char *out = malloc(cap), *buf = malloc(output_size);
    ZSTD_DCtx *ctx = ZSTD_createDCtx();
    if (!out || !buf || !ctx) return -1;
    unsigned log = 23;
    while (log < 27 && ((uint64_t)1 << log) < (uint64_t)dict_size * 5 / 4) log++;
    size_t rc = ZSTD_DCtx_setParameter(ctx, ZSTD_d_windowLogMax, (int)log);
    if (dict_size) rc = ZSTD_DCtx_loadDictionary_advanced(ctx, in + 12, dict_size, ZSTD_dlm_byRef, ZSTD_dct_rawContent);
    uint32_t status = ZSTD_isError(rc) ? 1 : 0;
    for (uint32_t i = 0; i < chunks; i++) {
        if (n - pos < 4) return -1;
        uint32_t len = get32(in + pos); pos += 4;
        if (len > n - pos) return -1;
        ZSTD_inBuffer src = { in + pos, len, 0 };
        size_t before = used;
        unsigned char field[4] = {0};
        if (append(&out, &used, &cap, field, 4)) return -1;
        if (!status) {
            size_t produced;
            do {
                ZSTD_outBuffer dst = {buf, output_size, 0};
                size_t previous = src.pos;
                rc = ZSTD_decompressStream(ctx, &dst, &src);
                if (ZSTD_isError(rc)) {
                    status = ZSTD_getErrorCode(rc) == ZSTD_error_frameParameter_windowTooLarge ? 2 : 1;
                    break;
                }
                if (append(&out, &used, &cap, buf, dst.pos)) return -1;
                produced = dst.pos;
                if (src.pos == previous && dst.pos == 0) break;
            } while (src.pos < src.size || produced == output_size);
        }
        put32(out + before, (uint32_t)(used - before - 4));
        pos += len;
    }
    if (pos != n) return -1;
    put32(out, status);
    ZSTD_freeDCtx(ctx); free(buf);
    *result = out; *size = (uint32_t)used;
    return 0;
}

int main(void) {
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
