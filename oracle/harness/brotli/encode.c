#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include "brotli/encode.h"

int main(int argc, char **argv) {
    if (argc != 3 && argc != 4) return 1;
    int quality = atoi(argv[1]), window = atoi(argv[2]);
    size_t size = 0, cap = 65536;
    uint8_t *data = malloc(cap);
    if (!data) return 1;
    for (;;) {
        if (size == cap) {
            cap *= 2;
            uint8_t *next = realloc(data, cap);
            if (!next) return 1;
            data = next;
        }
        size_t n = fread(data + size, 1, cap - size, stdin);
        size += n;
        if (!n) break;
    }
    if (ferror(stdin)) return 1;
    BrotliEncoderState *state = BrotliEncoderCreateInstance(NULL, NULL, NULL);
    if (!state || !BrotliEncoderSetParameter(state, BROTLI_PARAM_QUALITY, quality)) return 1;
    uint8_t *dictionary = NULL;
    BrotliEncoderPreparedDictionary *prepared = NULL;
    if (argc == 4) {
        FILE *file = fopen(argv[3], "rb");
        if (!file || fseek(file, 0, SEEK_END)) return 1;
        long length = ftell(file);
        if (length <= 0 || fseek(file, 0, SEEK_SET)) return 1;
        dictionary = malloc((size_t)length);
        if (!dictionary || fread(dictionary, 1, (size_t)length, file) != (size_t)length || fclose(file)) return 1;
        prepared = BrotliEncoderPrepareDictionary(BROTLI_SHARED_DICTIONARY_RAW, (size_t)length, dictionary, quality, NULL, NULL, NULL);
        if (!prepared || !BrotliEncoderAttachPreparedDictionary(state, prepared)) return 1;
    }
    if (window > 24 && !BrotliEncoderSetParameter(state, BROTLI_PARAM_LARGE_WINDOW, 1)) return 1;
    if (!BrotliEncoderSetParameter(state, BROTLI_PARAM_LGWIN, window)) return 1;
    const uint8_t *next_in = data;
    size_t available_in = size;
    while (!BrotliEncoderIsFinished(state)) {
        uint8_t output[65536], *next_out = output;
        size_t available_out = sizeof(output);
        if (!BrotliEncoderCompressStream(state, BROTLI_OPERATION_FINISH, &available_in, &next_in, &available_out, &next_out, NULL)) return 1;
        size_t produced = sizeof(output) - available_out;
        if (fwrite(output, 1, produced, stdout) != produced) return 1;
    }
    BrotliEncoderDestroyInstance(state);
    BrotliEncoderDestroyPreparedDictionary(prepared);
    free(dictionary);
    free(data);
    return fflush(stdout) ? 1 : 0;
}
