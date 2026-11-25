#include "url/gurl.h"
#include "url/origin.h"

int main() {
  GURL value("https://example.com/");
  return !value.is_valid() || url::Origin::Create(value).opaque();
}
