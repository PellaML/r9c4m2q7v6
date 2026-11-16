#ifndef GENERIC_METAL_PROBE_H
#define GENERIC_METAL_PROBE_H
#include <stddef.h>
#include <stdint.h>

enum { GP_SIDE = 32, GP_ROW = 256, GP_GUARD = 256,
       GP_SLAB = GP_GUARD + GP_ROW * GP_SIDE + GP_GUARD,
       GP_MAX_READBACK = 6 * GP_SLAB, GP_ABI = 1 };

/* All coordinates are independently authored, public synthetic inputs. */
typedef struct {
    uint32_t id, draw_count, vertex_start, index_first;
    uint32_t indexed, instances, primitive, keep;
    uint32_t clear_stencil, no_draw;
    float vertices[8][2];
    uint16_t indices[10];
    float offsets[3][2];
    float viewport[4];
    uint32_t scissor[4];
} GPCase;

typedef struct {
    uint32_t submitted, completion_seen, timed_out, status;
    uint32_t error_present, bytes, mode, reserved;
    int64_t error_code;
    double gpu_start, gpu_end;
    char reason[96];
    char error_domain[96];
} GPReceipt;

typedef struct {
    uint64_t registry_id;
    uint32_t paravirtual_name_match, unified_memory, low_power, removable;
    uint32_t available_modes, device_count, selected_by, sdk_max;
    char device_name[192];
    char os_version[96];
    char os_build[64];
    char architecture[32];
    char clang_version[192];
    GPReceipt mode_errors[2];
} GPMetadata;

int gp_check_abi(uint32_t abi, size_t case_size, size_t receipt_size, size_t metadata_size);
int gp_open(uint64_t requested_registry, void **handle, GPMetadata *metadata, GPReceipt *receipt);
int gp_run(void *handle, const GPCase *input, uint32_t mode, uint8_t *output,
           size_t capacity, GPReceipt *receipt);
void gp_close(void *handle);
#endif

