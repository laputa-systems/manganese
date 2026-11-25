#include <cstddef>
#include <cstdint>

#include "base/containers/span.h"
#include "base/hash/hash.h"
#include "third_party/leveldatabase/src/util/hash.h"

extern "C" uint32_t persistent_hash(const uint8_t *data, size_t len) {
    return base::PersistentHash(base::span(data, len));
}

extern "C" uint64_t fast_hash(const uint8_t *data, size_t len) {
    return base::FastHash(base::span(data, len));
}

extern "C" uint32_t leveldb_hash(const uint8_t *data, size_t len, uint32_t seed) {
    return leveldb::Hash(reinterpret_cast<const char *>(data), len, seed);
}
