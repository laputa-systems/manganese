#include <setjmp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "jpeglib.h"

struct encoder_error {
  struct jpeg_error_mgr pub;
  jmp_buf jump;
};

static void error_exit(j_common_ptr info) {
  (*info->err->output_message)(info);
  longjmp(((struct encoder_error *)info->err)->jump, 1);
}

static unsigned char *input;
static size_t input_size;
static size_t input_offset;

static uint32_t take32(void) {
  if (input_size - input_offset < 4) {
    fprintf(stderr, "encoder: truncated parameters\n");
    exit(2);
  }
  const unsigned char *p = input + input_offset;
  input_offset += 4;
  return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | p[3];
}

static const unsigned char *take(size_t length) {
  if (input_size - input_offset < length) {
    fprintf(stderr, "encoder: truncated data\n");
    exit(2);
  }
  const unsigned char *p = input + input_offset;
  input_offset += length;
  return p;
}

int main(void) {
  size_t capacity = 1 << 20;
  input = malloc(capacity);
  size_t got;
  while ((got = fread(input + input_size, 1, capacity - input_size, stdin)) > 0) {
    input_size += got;
    if (input_size == capacity) {
      capacity *= 2;
      input = realloc(input, capacity);
    }
  }
  if (input_size < 4 || memcmp(input, "JENC", 4) != 0) {
    fprintf(stderr, "encoder: bad magic\n");
    return 2;
  }
  input_offset = 4;

  struct jpeg_compress_struct info;
  struct encoder_error error;
  unsigned char *output = NULL;
  unsigned long output_size = 0;
  info.err = jpeg_std_error(&error.pub);
  error.pub.error_exit = error_exit;
  if (setjmp(error.jump)) return 3;
  jpeg_create_compress(&info);
  jpeg_mem_dest(&info, &output, &output_size);

  const uint32_t width = take32(), height = take32(), input_components = take32();
  const uint32_t in_color_space = take32(), jpeg_color_space = take32(), precision = take32();
  const uint32_t quality = take32(), optimize = take32(), progressive = take32(), smoothing = take32();
  const uint32_t restart_interval = take32(), restart_in_rows = take32(), dct_method = take32();
  const uint32_t write_jfif = take32(), write_adobe = take32(), predictor = take32(), point_transform = take32();
  info.image_width = width;
  info.image_height = height;
  info.input_components = (int)input_components;
  info.in_color_space = (J_COLOR_SPACE)in_color_space;
  info.data_precision = (int)precision;
  jpeg_set_defaults(&info);
  jpeg_set_colorspace(&info, (J_COLOR_SPACE)jpeg_color_space);
  jpeg_set_quality(&info, (int)quality, quality >= 25);
  if (predictor) jpeg_enable_lossless(&info, (int)predictor, (int)point_transform);
  info.optimize_coding = optimize != 0;
  info.smoothing_factor = (int)smoothing;
  info.restart_interval = restart_interval;
  info.restart_in_rows = (int)restart_in_rows;
  info.dct_method = (J_DCT_METHOD)dct_method;
  info.write_JFIF_header = write_jfif != 0;
  info.write_Adobe_marker = write_adobe != 0;
  for (int c = 0; c < info.num_components; ++c) {
    info.comp_info[c].h_samp_factor = (int)take32();
    info.comp_info[c].v_samp_factor = (int)take32();
    info.comp_info[c].quant_tbl_no = (int)take32();
  }
  const uint32_t scans = take32();
  jpeg_scan_info *script = scans ? calloc(scans, sizeof(jpeg_scan_info)) : NULL;
  for (uint32_t s = 0; s < scans; ++s) {
    script[s].comps_in_scan = (int)take32();
    for (int c = 0; c < MAX_COMPS_IN_SCAN; ++c) script[s].component_index[c] = (int)take32();
    script[s].Ss = (int)take32();
    script[s].Se = (int)take32();
    script[s].Ah = (int)take32();
    script[s].Al = (int)take32();
  }
  if (scans) {
    info.scan_info = script;
    info.num_scans = (int)scans;
  } else if (progressive) {
    jpeg_simple_progression(&info);
  }
  const uint32_t markers = take32();
  const size_t markers_offset = input_offset;
  for (uint32_t m = 0; m < markers; ++m) {
    take32();
    take(take32());
  }
  const uint32_t icc_size = take32();
  const unsigned char *icc = take(icc_size);
  const size_t sample_bytes = precision > 8 ? 2 : 1;
  const size_t row_samples = (size_t)width * input_components;
  const unsigned char *pixels = take(row_samples * height * sample_bytes);

  jpeg_start_compress(&info, TRUE);
  size_t marker_offset = markers_offset;
  for (uint32_t m = 0; m < markers; ++m) {
    input_offset = marker_offset;
    const uint32_t code = take32();
    const uint32_t length = take32();
    jpeg_write_marker(&info, (int)code, take(length), length);
    marker_offset = input_offset;
  }
  if (icc_size) jpeg_write_icc_profile(&info, icc, icc_size);
  if (precision <= 8) {
    while (info.next_scanline < info.image_height) {
      JSAMPROW row = (JSAMPROW)(pixels + (size_t)info.next_scanline * row_samples);
      jpeg_write_scanlines(&info, &row, 1);
    }
  } else {
    unsigned short *wide = malloc(row_samples * sizeof(unsigned short));
    while (info.next_scanline < info.image_height) {
      const unsigned char *source = pixels + (size_t)info.next_scanline * row_samples * 2;
      for (size_t i = 0; i < row_samples; ++i) wide[i] = (unsigned short)((source[2 * i] << 8) | source[2 * i + 1]);
      if (precision <= 12) {
        J12SAMPROW row = (J12SAMPROW)wide;
        jpeg12_write_scanlines(&info, &row, 1);
      } else {
        J16SAMPROW row = (J16SAMPROW)wide;
        jpeg16_write_scanlines(&info, &row, 1);
      }
    }
    free(wide);
  }
  jpeg_finish_compress(&info);
  jpeg_destroy_compress(&info);
  if (fwrite(output, 1, output_size, stdout) != output_size || fflush(stdout)) return 1;
  return 0;
}
