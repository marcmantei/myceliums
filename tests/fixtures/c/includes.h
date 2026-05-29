#ifndef INCLUDES_H
#define INCLUDES_H

#include <stdint.h>
#include <stdbool.h>
#include "types.h"

#define VERSION_MAJOR 1
#define VERSION_MINOR 0
#define MAX(a, b) ((a) > (b) ? (a) : (b))

typedef int (*compare_fn)(const void *, const void *);

struct Config {
    int timeout;
    bool verbose;
    const char *name;
};

enum LogLevel {
    LOG_DEBUG,
    LOG_INFO,
    LOG_WARN,
    LOG_ERROR
};

// Forward declarations
int init(struct Config *config);
void cleanup(void);
int compare_items(const void *a, const void *b);

#endif
