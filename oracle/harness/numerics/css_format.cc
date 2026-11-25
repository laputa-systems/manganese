#include <stddef.h>
#include <math.h>
#include <string.h>
#include <string>
#include "absl/strings/str_format.h"

extern "C" size_t css_format(double value, char* output) {
  std::string text = isnan(value) ? "NaN" : isinf(value) ? (value < 0 ? "-infinity" : "infinity") : absl::StrFormat("%.6g", value);
  memcpy(output, text.data(), text.size());
  return text.size();
}
