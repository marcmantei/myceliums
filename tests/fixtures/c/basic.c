#include <stdio.h>
#include <stdlib.h>
#include "myheader.h"

#define MAX_SIZE 1024
#define SQUARE(x) ((x) * (x))

typedef unsigned long ulong;

typedef struct {
    int x;
    int y;
} Point;

struct Node {
    int value;
    struct Node *next;
};

union Data {
    int i;
    float f;
    char c;
};

enum Color {
    RED,
    GREEN,
    BLUE
};

int add(int a, int b) {
    return a + b;
}

void *allocate(size_t size) {
    return malloc(size);
}

void process(struct Node *node) {
    int result = add(node->value, 1);
    printf("Result: %d\n", result);
    free(node);
}

int main(int argc, char *argv[]) {
    struct Node *n = allocate(sizeof(struct Node));
    n->value = 42;
    n->next = NULL;
    process(n);
    return 0;
}
