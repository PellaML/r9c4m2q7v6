#pragma once
#include <stddef.h>
#include <stdint.h>

typedef struct {
    uint32_t count, linear, zero_axes, pixel_coords, use_constexpr, token;
    uint8_t rgba[4];
    float bias;
} ProbeConfig;

typedef struct {
    uint32_t rgba[4];
    uint32_t stamp;
    uint32_t reserved[3];
} ProbeSample;

typedef struct {
    uint32_t wait_called, command_status, texture_verified;
    uint32_t upload_encoded, render_encoded, readback_copied;
} ProbeRunInfo;

// 0: completed, 1: failed, 2: a requested native resource is unavailable.
int32_t probe_open(void **context, char *json, size_t capacity);
int32_t probe_sample(void *context, const char *library_path, const char *function_name,
                     const uint32_t *coordinates, const ProbeConfig *config,
                     ProbeSample *samples, ProbeRunInfo *info, char *json, size_t capacity);
void probe_close(void *context);
