#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "brotli/decode.h"

static uint32_t get32(const unsigned char *p) {
    return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | p[3];
}

static void put32(unsigned char *p, uint32_t n) {
    p[0] = n >> 24; p[1] = n >> 16; p[2] = n >> 8; p[3] = n;
}

static int append(unsigned char **result, size_t *used, size_t *capacity, int32_t code, const unsigned char *data, size_t len) {
    size_t required = *used + 4 + len;
    if (required > UINT32_MAX) return -1;
    if (required > *capacity) {
        size_t next_cap = *capacity;
        while (next_cap < required) next_cap *= 2;
        unsigned char *next = realloc(*result, next_cap);
        if (!next) return -1;
        *result = next; *capacity = next_cap;
    }
    put32(*result + *used, (uint32_t)code);
    if (len) memcpy(*result + *used + 4, data, len);
    *used = required;
    return 0;
}

static int record(const unsigned char *in, uint32_t n, unsigned char **result, uint32_t *size) {
    if (n < 11 || in[0] > 3) return -1;
    unsigned mode = in[0], chunks = ((unsigned)in[1] << 8) | in[2];
    uint32_t output_size = get32(in + 3), dict_size = get32(in + 7);
    size_t pos = 11, used = 0, cap = 4096;
    if ((!output_size && !mode) || output_size > 16 * 1024 * 1024 || dict_size > n - pos || (mode && (dict_size || chunks != 1)) || (mode == 3 && output_size != 32768)) return -1;
    unsigned char *response = malloc(cap), *output = calloc(output_size ? output_size : 1, 1);
    if (!response || !output) return -1;
    response[used++] = mode;
    BrotliDecoderState *state = NULL;
    if (!mode) {
        state = BrotliDecoderCreateInstance(NULL, NULL, NULL);
        if (!state) return -1;
        if (dict_size && !BrotliDecoderAttachDictionary(state, BROTLI_SHARED_DICTIONARY_RAW, dict_size, in + pos)) return -1;
    }
    pos += dict_size;
    int finished = 0;
    for (unsigned i = 0; i < chunks; i++) {
        if (n - pos < 4) return -1;
        uint32_t len = get32(in + pos); pos += 4;
        if (len > n - pos || (!mode && len > 32768)) return -1;
        const uint8_t *next_in = in + pos;
        size_t available_in = len;
        pos += len;
        if (mode == 1 || mode == 2) {
            size_t decoded_size = output_size;
            BrotliDecoderResult status = BrotliDecoderDecompress(len, next_in, &decoded_size, output);
            int success = mode == 2 || (status == BROTLI_DECODER_RESULT_SUCCESS && decoded_size == output_size);
            if (append(&response, &used, &cap, success, output, success ? output_size : 0)) return -1;
            continue;
        }
        if (mode == 3) {
            state = BrotliDecoderCreateInstance(NULL, NULL, NULL);
            if (!state) return -1;
            if (append(&response, &used, &cap, 1, NULL, 0)) return -1;
            for (;;) {
                uint8_t *next_out = output;
                size_t available_out = output_size;
                BrotliDecoderResult status = BrotliDecoderDecompressStream(state, &available_in, &next_in, &available_out, &next_out, NULL);
                if (status == BROTLI_DECODER_RESULT_ERROR || status == BROTLI_DECODER_RESULT_NEEDS_MORE_INPUT) {
                    used = 1; if (append(&response, &used, &cap, 0, NULL, 0)) return -1; break;
                }
                size_t produced = output_size - available_out;
                if (used + produced > UINT32_MAX) return -1;
                if (used + produced > cap) {
                    while (used + produced > cap) cap *= 2;
                    unsigned char *next = realloc(response, cap);
                    if (!next) return -1;
                    response = next;
                }
                memcpy(response + used, output, produced); used += produced;
                if (status == BROTLI_DECODER_RESULT_SUCCESS) {
                    if (available_in) { used = 1; if (append(&response, &used, &cap, -1, NULL, 0)) return -1; }
                    break;
                }
            }
            BrotliDecoderDestroyInstance(state);
            continue;
        }
        if (finished) continue;
        for (;;) {
            uint8_t *next_out = output;
            size_t available_out = output_size;
            BrotliDecoderResult status = BrotliDecoderDecompressStream(state, &available_in, &next_in, &available_out, &next_out, NULL);
            size_t produced = output_size - available_out;
            if (status == BROTLI_DECODER_RESULT_ERROR) {
                if (append(&response, &used, &cap, -330, NULL, 0)) return -1;
                finished = 2; break;
            }
            if (produced && append(&response, &used, &cap, (int32_t)produced, output, produced)) return -1;
            if (status == BROTLI_DECODER_RESULT_SUCCESS) { finished = 1; break; }
            if (status == BROTLI_DECODER_RESULT_NEEDS_MORE_INPUT) break;
        }
    }
    if (pos != n) return -1;
    if (!mode) {
        if (!finished) {
            const uint8_t *next_in = in + n;
            size_t available_in = 0;
            for (;;) {
                uint8_t *next_out = output;
                size_t available_out = output_size;
                BrotliDecoderResult status = BrotliDecoderDecompressStream(state, &available_in, &next_in, &available_out, &next_out, NULL);
                size_t produced = output_size - available_out;
                if (status == BROTLI_DECODER_RESULT_ERROR) {
                    if (append(&response, &used, &cap, -330, NULL, 0)) return -1;
                    finished = 2; break;
                }
                if (produced && append(&response, &used, &cap, (int32_t)produced, output, produced)) return -1;
                if (status != BROTLI_DECODER_RESULT_NEEDS_MORE_OUTPUT) break;
            }
        }
        if (finished != 2 && append(&response, &used, &cap, 0, NULL, 0)) return -1;
        BrotliDecoderDestroyInstance(state);
    }
    free(output);
    *result = response; *size = (uint32_t)used;
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
