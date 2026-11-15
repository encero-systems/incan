#ifndef PROBE_H
#define PROBE_H

struct probe_pair {
    int first;
#if defined(__linux__) && PROBE_WIDE
    int padding;
#endif
    int second;
};

int abs(int value);

#endif
