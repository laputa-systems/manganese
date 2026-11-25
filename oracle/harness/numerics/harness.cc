#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "base/third_party/double_conversion/double-conversion/double-conversion.h"
#include "third_party/simdutf/simdutf.h"
#include "base/strings/string_number_conversions_internal.h"
#include "base/strings/string_util_impl_helpers.h"
#include "base/strings/utf_string_conversion_utils.h"
#include "third_party/blink/renderer/platform/wtf/dtoa.h"

using double_conversion::DoubleToStringConverter;
using double_conversion::StringBuilder;
using double_conversion::StringToDoubleConverter;

static uint32_t read_u32(const unsigned char* p) {
  return (uint32_t(p[0]) << 24) | (uint32_t(p[1]) << 16) | (uint32_t(p[2]) << 8) | p[3];
}

static void write_u32(unsigned char* p, uint32_t v) {
  p[0] = v >> 24; p[1] = v >> 16; p[2] = v >> 8; p[3] = v;
}

template <typename T, int base>
static void parse_integer(const unsigned char* input, size_t length, unsigned char* output) {
  std::string_view text(reinterpret_cast<const char*>(input), length);
  auto result = base::internal::StringToNumber<T, base>(text);
  output[0] = result.valid;
  uint64_t value = static_cast<uint64_t>(result.value);
  for (int i = 0; i < 8; ++i) output[1 + i] = value >> (56 - 8 * i);
}

int main() {
  char magic[4];
  if (fread(magic, 1, 4, stdin) != 4 || memcmp(magic, "MGB1", 4)) return 1;
  if (fwrite("MGB1", 1, 4, stdout) != 4) return 1;
  fflush(stdout);
  unsigned char header[4];
  while (fread(header, 1, 4, stdin) == 4) {
    uint32_t length = read_u32(header);
    if (length > 65536 || length < 1) return 1;
    unsigned char* input = static_cast<unsigned char*>(malloc(length));
    if (!input || fread(input, 1, length, stdin) != length) return 1;
    unsigned char output[196608] = {};
    size_t out_len = 0;
    if (input[0] <= 4 && length == 11) {
      uint64_t bits = 0;
      for (int i = 1; i < 9; ++i) bits = (bits << 8) | input[i];
      double value;
      memcpy(&value, &bits, sizeof(value));
      int digits = int(input[9]) * 256 + input[10] - 1;
      char text[1024];
      StringBuilder builder(text, sizeof(text));
      DoubleToStringConverter base(DoubleToStringConverter::EMIT_POSITIVE_EXPONENT_SIGN, nullptr, nullptr, 'e', -6, 12, 0, 0);
      const auto& ecma = DoubleToStringConverter::EcmaScriptConverter();
      bool success = false;
      if (input[0] == 0) success = base.ToShortest(value, &builder);
      if (input[0] == 1 && digits >= 0) success = base.ToFixed(value, digits, &builder);
      if (input[0] == 2) success = ecma.ToShortest(value, &builder);
      if (input[0] == 3) success = ecma.ToPrecision(value, digits, &builder);
      if (input[0] == 4) success = ecma.ToExponential(value, digits, &builder);
      output[0] = success;
      out_len = 1 + builder.position();
      memcpy(output + 1, text, out_len - 1);
      builder.Finalize();
    } else if ((input[0] == 10 || input[0] == 11) && length == 11) {
      uint64_t bits = 0;
      for (int i = 1; i < 9; ++i) bits = (bits << 8) | input[i];
      double value;
      memcpy(&value, &bits, sizeof(value));
      unsigned digits = (unsigned(input[9]) << 8) | input[10];
      if (digits > 20 || (input[0] == 10 && digits == 0)) return 1;
      blink::DoubleToStringConverter converter;
      auto result = input[0] == 10 ? converter.ToStringWithFixedPrecision(value, digits) : converter.ToStringWithFixedWidth(value, digits);
      out_len = result.size();
      memcpy(output, result.data(), out_len);
    } else if (input[0] == 9) {
      std::string_view text(reinterpret_cast<char*>(input + 1), length - 1);
      output[0] = base::internal::DoIsStringUTF8<base::IsValidCharacter>(text);
      output[1] = base::internal::DoIsStringUTF8<base::IsValidCodepoint>(text);
      out_len = 2;
    } else if (input[0] == 7 && length >= 2) {
      switch (input[1]) {
        case 0: parse_integer<int32_t, 10>(input + 2, length - 2, output); break;
        case 1: parse_integer<uint32_t, 10>(input + 2, length - 2, output); break;
        case 2: parse_integer<int64_t, 10>(input + 2, length - 2, output); break;
        case 3: parse_integer<uint64_t, 10>(input + 2, length - 2, output); break;
        case 4: parse_integer<int32_t, 16>(input + 2, length - 2, output); break;
        case 5: parse_integer<uint32_t, 16>(input + 2, length - 2, output); break;
        case 6: parse_integer<int64_t, 16>(input + 2, length - 2, output); break;
        case 7: parse_integer<uint64_t, 16>(input + 2, length - 2, output); break;
        default: return 1;
      }
      out_len = 9;
    } else if (input[0] == 8) {
      std::string_view text(reinterpret_cast<char*>(input + 1), length - 1);
      double value;
      output[0] = base::internal::StringToDoubleImpl(text, text.data(), value);
      uint64_t bits;
      memcpy(&bits, &value, sizeof(bits));
      for (int i = 0; i < 8; ++i) output[1 + i] = bits >> (56 - 8 * i);
      out_len = 9;
    } else if (input[0] == 6 && length % 2 == 1) {
      size_t count = (length - 1) / 2;
      char16_t* units = static_cast<char16_t*>(malloc((count + 1) * sizeof(char16_t)));
      if (!units) return 1;
      for (size_t i = 0; i < count; ++i) units[i] = (uint16_t(input[1 + i * 2]) << 8) | input[2 + i * 2];
      simdutf::result result = simdutf::convert_utf16_to_utf8_with_errors(units, count, reinterpret_cast<char*>(output + 8));
      write_u32(output, result.error);
      write_u32(output + 4, result.count);
      if (result.error == simdutf::SUCCESS) out_len = 8 + result.count;
      else {
        size_t prefix = simdutf::convert_utf16_to_utf8(units, result.count, reinterpret_cast<char*>(output + 8));
        out_len = 8 + prefix;
      }
      free(units);
    } else if (input[0] == 5) {
      StringToDoubleConverter converter(StringToDoubleConverter::ALLOW_LEADING_SPACES | StringToDoubleConverter::ALLOW_TRAILING_JUNK, 0.0, 0.0, nullptr, nullptr);
      int consumed = 0;
      double value = converter.StringToDouble(reinterpret_cast<char*>(input + 1), length - 1, &consumed);
      uint64_t bits;
      memcpy(&bits, &value, sizeof(bits));
      write_u32(output, consumed);
      for (int i = 0; i < 8; ++i) output[4 + i] = bits >> (56 - 8 * i);
      out_len = 12;
    } else {
      free(input);
      return 1;
    }
    free(input);
    write_u32(header, out_len);
    if (fwrite(header, 1, 4, stdout) != 4 || fwrite(output, 1, out_len, stdout) != out_len) return 1;
    fflush(stdout);
  }
  return feof(stdin) ? 0 : 1;
}
