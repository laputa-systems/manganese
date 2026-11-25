#include <stdlib.h>
#include "partition_alloc/pointers/raw_ptr_backup_ref_impl.h"

namespace base::internal {
template <>
void RawPtrBackupRefImpl<false>::AcquireInternal(uintptr_t) { abort(); }
template <>
void RawPtrBackupRefImpl<false>::ReleaseInternal(uintptr_t) { abort(); }
}
