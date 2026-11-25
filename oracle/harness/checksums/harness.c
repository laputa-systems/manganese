#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "third_party/crc32c/src/include/crc32c/crc32c.h"

uint32_t persistent_hash(const uint8_t *data, size_t len);
uint64_t fast_hash(const uint8_t *data, size_t len);
uint32_t leveldb_hash(const uint8_t *data, size_t len, uint32_t seed);

static uint32_t read32(const uint8_t *p) {
    return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | p[3];
}

static void write32(uint8_t *p, uint32_t n) {
    p[0] = n >> 24;
    p[1] = n >> 16;
    p[2] = n >> 8;
    p[3] = n;
}

static int record(const uint8_t *input, size_t size, uint8_t *output, size_t *output_size) {
    if (size < 8 || input[0] > 3 || input[1] > 63) return 0;
    unsigned op = input[0];
    unsigned offset = input[1];
    unsigned chunks = ((unsigned)input[2] << 8) | input[3];
    uint32_t checksum = read32(input + 4);
    size_t pos = 8;
    uint8_t *combined = malloc(size + offset + 63);
    if (!combined) return 0;
    size_t base = (64 - ((uintptr_t)combined % 64)) % 64 + offset;
    size_t total = 0;
    for (unsigned i = 0; i < chunks; i++) {
        if (size - pos < 4) { free(combined); return 0; }
        uint32_t len = read32(input + pos);
        pos += 4;
        if (len > size - pos) { free(combined); return 0; }
        uint8_t *data = malloc((size_t)len + offset + 63);
        if (!data) { free(combined); return 0; }
        size_t chunk_base = (64 - ((uintptr_t)data % 64)) % 64 + offset;
        memcpy(data + chunk_base, input + pos, len);
        if (op == 0) checksum = crc32c_extend(checksum, data + chunk_base, len);
        else { memcpy(combined + base + total, data + chunk_base, len); total += len; }
        free(data);
        pos += len;
    }
    if (pos != size) { free(combined); return 0; }
    uint64_t result = 0;
    if (op == 0) result = checksum;
    else if (op == 1) result = persistent_hash(combined + base, total);
    else if (op == 2) result = fast_hash(combined + base, total);
    else result = leveldb_hash(combined + base, total, checksum);
    *output_size = op == 2 ? 8 : 4;
    if (*output_size == 8) write32(output, result >> 32);
    write32(output + *output_size - 4, (uint32_t)result);
    free(combined);
    return 1;
}

int main(void) {
    uint8_t header[4];
    if (fread(header, 1, 4, stdin) != 4 || memcmp(header, "MGB1", 4)) return 1;
    if (fwrite("MGB1", 1, 4, stdout) != 4 || fflush(stdout)) return 1;
    while (fread(header, 1, 1, stdin) == 1) {
        if (fread(header + 1, 1, 3, stdin) != 3) return 1;
        uint32_t size = read32(header);
        uint8_t *input = malloc((size_t)size + 1);
        if (!input) return 1;
        if (fread(input, 1, size, stdin) != size) { free(input); return 1; }
        uint8_t *output = malloc((size_t)size * 2 + 8);
        if (!output) { free(input); return 1; }
        size_t output_size;
        int valid = record(input, size, output, &output_size);
        free(input);
        if (!valid) { free(output); return 1; }
        write32(header, output_size);
        if (fwrite(header, 1, 4, stdout) != 4 || fwrite(output, 1, output_size, stdout) != output_size || fflush(stdout)) { free(output); return 1; }
        free(output);
    }
    return ferror(stdin) ? 1 : 0;
}
