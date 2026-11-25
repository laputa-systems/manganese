#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "brotli/decode.h"
#include "brotli/shared_dictionary.h"

enum { STREAM, ONESHOT_EXACT, ONESHOT_BUFFER, PATCH, SHARED, SHARED_DICT };

typedef struct {
    unsigned char *data;
    size_t used, cap;
} buffer;

static uint32_t get32(const unsigned char *p) {
    return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | p[3];
}

static void put32(unsigned char *p, uint32_t n) {
    p[0] = n >> 24; p[1] = n >> 16; p[2] = n >> 8; p[3] = n;
}

static int put(buffer *b, const void *data, size_t len) {
    if (len > UINT32_MAX - b->used) return -1;
    size_t required = b->used + len;
    if (required > b->cap) {
        size_t next_cap = b->cap ? b->cap : 4096;
        while (next_cap < required) next_cap *= 2;
        unsigned char *next = realloc(b->data, next_cap);
        if (!next) return -1;
        b->data = next;
        b->cap = next_cap;
    }
    if (len) memcpy(b->data + b->used, data, len);
    b->used = required;
    return 0;
}

static int put_code(buffer *b, int32_t code) {
    unsigned char word[4];
    put32(word, (uint32_t)code);
    return put(b, word, 4);
}

typedef struct {
    const unsigned char *data;
    uint32_t len;
} chunk;

static int stream(buffer *resp, BrotliDecoderState *state, const chunk *chunks, unsigned count, uint8_t *output, size_t output_size) {
    int done = 0;
    for (unsigned i = 0; i <= count; i++) {
        const uint8_t *next_in = chunks[i].data;
        size_t available_in = chunks[i].len;
        while (!done) {
            uint8_t *next_out = output;
            size_t available_out = output_size;
            BrotliDecoderResult status = BrotliDecoderDecompressStream(state, &available_in, &next_in, &available_out, &next_out, NULL);
            size_t produced = output_size - available_out;
            if (status == BROTLI_DECODER_RESULT_ERROR) return put_code(resp, -330);
            if (produced && (put_code(resp, (int32_t)produced) || put(resp, output, produced))) return -1;
            if (status == BROTLI_DECODER_RESULT_SUCCESS) { done = 1; break; }
            if (!produced) break;
        }
    }
    return put_code(resp, 0);
}

static int oneshot(buffer *resp, unsigned mode, const chunk *input, uint8_t *output, size_t output_size) {
    size_t decoded = output_size;
    BrotliDecoderResult status = BrotliDecoderDecompress(input->len, input->data, &decoded, output);
    int ok = status == BROTLI_DECODER_RESULT_SUCCESS && (mode == ONESHOT_BUFFER || decoded == output_size);
    if (put_code(resp, ok)) return -1;
    return ok ? put(resp, output, output_size) : 0;
}

static int patch(buffer *resp, BrotliDecoderState *state, const chunk *input, uint8_t *output, size_t output_size) {
    buffer body = {0};
    const uint8_t *next_in = input->data;
    size_t available_in = input->len;
    int code = 1;
    while (available_in || !BrotliDecoderIsFinished(state)) {
        uint8_t *next_out = output;
        size_t available_out = output_size, before = available_in;
        BrotliDecoderResult status = BrotliDecoderDecompressStream(state, &available_in, &next_in, &available_out, &next_out, NULL);
        size_t produced = output_size - available_out;
        if (status == BROTLI_DECODER_RESULT_ERROR || status == BROTLI_DECODER_RESULT_NEEDS_MORE_INPUT) { code = 0; break; }
        if (status == BROTLI_DECODER_RESULT_SUCCESS && available_in == before && !produced) { code = -1; break; }
        if (put(&body, output, produced)) { free(body.data); return -1; }
    }
    int failed = put_code(resp, code) || (code == 1 && put(resp, body.data, body.used));
    free(body.data);
    return failed ? -1 : 0;
}

static int shared(buffer *resp, BrotliDecoderState *state, unsigned mode, const unsigned char *dictionary, size_t dict_size, const chunk *input, uint8_t *output, size_t output_size) {
    if (mode == SHARED_DICT && !BrotliDecoderAttachDictionary(state, BROTLI_SHARED_DICTIONARY_RAW, dict_size, dictionary)) return put_code(resp, 1);
    const uint8_t *next_in = input->data;
    size_t available_in = input->len, available_out = output_size, total_out = 0;
    uint8_t *next_out = output;
    for (;;) {
        BrotliDecoderResult status = BrotliDecoderDecompressStream(state, &available_in, &next_in, &available_out, &next_out, &total_out);
        if (status == BROTLI_DECODER_RESULT_SUCCESS) break;
        if (status == BROTLI_DECODER_RESULT_ERROR) return put_code(resp, 2);
        if (status == BROTLI_DECODER_RESULT_NEEDS_MORE_INPUT && !available_in) return put_code(resp, 2);
        if (status == BROTLI_DECODER_RESULT_NEEDS_MORE_OUTPUT && !available_out) return put_code(resp, 3);
    }
    if (available_in) return put_code(resp, 4);
    if (total_out > output_size) return put_code(resp, 3);
    return put_code(resp, 0) || put(resp, output, total_out) ? -1 : 0;
}

static int record(const unsigned char *in, uint32_t n, buffer *resp) {
    if (n < 11 || in[0] > SHARED_DICT) return -1;
    unsigned mode = in[0], count = ((unsigned)in[1] << 8) | in[2];
    uint32_t output_size = get32(in + 3), dict_size = get32(in + 7);
    size_t pos = 11;
    if (output_size > 16 * 1024 * 1024 || dict_size > n - pos) return -1;
    if (mode == STREAM ? !output_size : (count != 1 || (mode != SHARED_DICT && dict_size))) return -1;
    if (mode == PATCH && output_size != 32768) return -1;
    const unsigned char *dictionary = in + pos;
    pos += dict_size;
    chunk *chunks = calloc(count + 1, sizeof(chunk));
    uint8_t *output = calloc(output_size ? output_size : 1, 1);
    if (!chunks || !output) return -1;
    for (unsigned i = 0; i < count; i++) {
        if (n - pos < 4) return -1;
        uint32_t len = get32(in + pos);
        pos += 4;
        if (len > n - pos || (mode == STREAM && (!len || len > 32768))) return -1;
        chunks[i].data = in + pos;
        chunks[i].len = len;
        pos += len;
    }
    if (pos != n) return -1;
    chunks[count].data = in + n;
    unsigned char tag = (unsigned char)mode;
    if (put(resp, &tag, 1)) return -1;
    int failed = 0;
    if (mode == ONESHOT_EXACT || mode == ONESHOT_BUFFER) {
        failed = oneshot(resp, mode, &chunks[0], output, output_size);
    } else {
        BrotliDecoderState *state = BrotliDecoderCreateInstance(NULL, NULL, NULL);
        if (!state) return -1;
        if (mode == STREAM && dict_size && !BrotliDecoderAttachDictionary(state, BROTLI_SHARED_DICTIONARY_RAW, dict_size, dictionary)) return -1;
        if (mode == STREAM) failed = stream(resp, state, chunks, count, output, output_size);
        else if (mode == PATCH) failed = patch(resp, state, &chunks[0], output, output_size);
        else failed = shared(resp, state, mode, dictionary, dict_size, &chunks[0], output, output_size);
        BrotliDecoderDestroyInstance(state);
    }
    free(chunks);
    free(output);
    return failed;
}

int main(void) {
    unsigned char head[4];
    if (fread(head, 1, 4, stdin) != 4 || memcmp(head, "MGB1", 4)) return 1;
    if (fwrite("MGB1", 1, 4, stdout) != 4 || fflush(stdout)) return 1;
    while (fread(head, 1, 4, stdin) == 4) {
        uint32_t n = get32(head);
        unsigned char *in = malloc(n ? n : 1);
        buffer resp = {0};
        if (!in || fread(in, 1, n, stdin) != n || record(in, n, &resp)) return 1;
        free(in);
        put32(head, (uint32_t)resp.used);
        if (fwrite(head, 1, 4, stdout) != 4 || fwrite(resp.data, 1, resp.used, stdout) != resp.used || fflush(stdout)) return 1;
        free(resp.data);
    }
    return ferror(stdin) ? 1 : 0;
}
