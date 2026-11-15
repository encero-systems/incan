#include "shim.h"

template <typename T>
static T doubled(T value) {
    return value + value;
}

extern "C" int shim_twice(int value) {
    return doubled(value);
}
