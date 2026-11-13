#include "shim.h"
#include "offset.h"

int shim_scale(int value) {
    return value * SHIM_FACTOR + LOCAL_OFFSET;
}
