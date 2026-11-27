#include <cstddef>
#include <cstdlib>

void* operator new(std::size_t size) { return std::malloc(size); }
void* operator new[](std::size_t size) { return std::malloc(size); }
void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept { std::free(pointer); }
void operator delete[](void* pointer, std::size_t) noexcept { std::free(pointer); }

extern "C" int __cxa_guard_acquire(void*) { return 1; }
extern "C" void __cxa_guard_release(void*) {}
extern "C" void __cxa_guard_abort(void*) {}
