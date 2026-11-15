#ifndef SHIM_H
#define SHIM_H

#ifdef __cplusplus
extern "C" {
#endif

int shim_scale(int value);
int shim_twice(int value);

#ifdef SHIM_SCALE
#define SHIM_FACTOR SHIM_SCALE
#else
#define SHIM_FACTOR 1
#endif

#ifdef __cplusplus
}
#endif

#endif
