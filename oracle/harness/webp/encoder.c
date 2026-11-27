#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include "webp/encode.h"

static uint64_t next(uint64_t* state) {
  uint64_t x = *state;
  x ^= x >> 12;
  x ^= x << 25;
  x ^= x >> 27;
  *state = x;
  return x * 2685821657736338717ULL;
}

int main(int argc, char** argv) {
  if (argc != 6) return 2;
  uint64_t seed = strtoull(argv[1], NULL, 10);
  int width = atoi(argv[2]);
  int height = atoi(argv[3]);
  int lossless = atoi(argv[4]);
  int quality = atoi(argv[5]);
  if (width <= 0 || height <= 0 || (uint64_t)width * height > (1u << 20)) return 2;
  uint8_t* rgba = malloc((size_t)width * height * 4);
  if (!rgba) return 2;
  uint64_t state = seed | 1;
  int alpha_mode = (int)(next(&state) % 4);
  for (int i = 0; i < width * height; ++i) {
    uint64_t value = next(&state);
    uint8_t r = (uint8_t)(value >> 24);
    uint8_t g = (uint8_t)(value >> 32);
    uint8_t b = (uint8_t)(value >> 40);
    uint8_t a = (uint8_t)(value >> 48);
    if (alpha_mode == 0) a = 255;
    if (alpha_mode == 1) a = (uint8_t)((i / width) * 255 / height);
    if (alpha_mode == 2) a = (uint8_t)((i % width) * 255 / width);
    rgba[4 * i] = r;
    rgba[4 * i + 1] = g;
    rgba[4 * i + 2] = b;
    rgba[4 * i + 3] = a;
  }
  uint8_t* output = NULL;
  size_t size = lossless
                    ? WebPEncodeLosslessRGBA(rgba, width, height, width * 4, &output)
                    : WebPEncodeRGBA(rgba, width, height, width * 4, (float)quality, &output);
  free(rgba);
  if (!size || !output) return 3;
  if (fwrite(output, 1, size, stdout) != size) {
    free(output);
    return 4;
  }
  free(output);
  return 0;
}
