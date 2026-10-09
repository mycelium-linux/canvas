#include <stddef.h>
#include <stdint.h>
#define CANVAS_TVOS_ABI_VERSION 1
#define CANVAS_TVOS_MAX_FRAME 65536
/* Caller-owned, nonoverlapping buffers; no pointer retention or allocation. */
intptr_t canvas_tvos_request(const uint8_t *input, size_t input_len,
                            uint8_t *output, size_t output_capacity);
