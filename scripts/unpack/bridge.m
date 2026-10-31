#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

typedef struct {
    uint32_t word, index;
    uint16_t bits[4];
    uint32_t stamp, reserved;
} Sample;
_Static_assert(sizeof(Sample) == 24, "sample stride");
_Static_assert(offsetof(Sample, bits) == 8, "half bit offset");
_Static_assert(offsetof(Sample, stamp) == 16, "stamp offset");

static int finish(NSMutableDictionary *metadata, NSString *phase, NSError *error,
                  int status, char *json, size_t capacity) {
    metadata[@"phase"] = phase;
    // Never serialize paths, environment, usernames or localized diagnostics.
    metadata[@"error"] = error ? @{ @"domain": error.domain, @"code": @(error.code) }
                                : (id)[NSNull null];
    NSData *data = [NSJSONSerialization dataWithJSONObject:metadata
                    options:NSJSONWritingSortedKeys error:NULL];
    if (!data || data.length >= capacity) return 3;
    memcpy(json, data.bytes, data.length);
    json[data.length] = 0;
    return status;
}

int unpack_run(const char *library_path, const uint32_t *words, uint32_t count,
               Sample *samples, uint32_t info[4], char *json, size_t capacity) {
    memset(info, 0, 4 * sizeof(uint32_t));
    @autoreleasepool {
        NSMutableDictionary *metadata = [NSMutableDictionary dictionary];
        @try {
            metadata[@"api"] = @"Metal compute";
            metadata[@"availability"] = @"not_executed";
            metadata[@"os_version_and_build"] = NSProcessInfo.processInfo.operatingSystemVersionString;
            if (!words || !samples || count != 1040 || !library_path)
                return finish(metadata, @"invalid_arguments", nil, 3, json, capacity);
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (!device) {
                metadata[@"availability"] = @"unavailable";
                return finish(metadata, @"no_metal_device", nil, 2, json, capacity);
            }
            metadata[@"device_name"] = device.name ?: @"unknown";
            metadata[@"unified_memory"] = @(device.hasUnifiedMemory);
            if (!device.hasUnifiedMemory)
                return finish(metadata, @"requires_unified_memory", nil, 2, json, capacity);
            NSError *error = nil;
            NSURL *url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:library_path]];
            id<MTLLibrary> library = [device newLibraryWithURL:url error:&error];
            if (!library) return finish(metadata, @"load_metallib", error, 3, json, capacity);
            id<MTLFunction> function = [library newFunctionWithName:@"unpack_probe"];
            if (!function) return finish(metadata, @"missing_entry", nil, 3, json, capacity);
            id<MTLComputePipelineState> pipeline = [device newComputePipelineStateWithFunction:function error:&error];
            if (!pipeline) return finish(metadata, @"compute_pipeline", error, 3, json, capacity);
            info[0] = 1;
            id<MTLCommandQueue> queue = [device newCommandQueue];
            id<MTLBuffer> input = [device newBufferWithBytes:words length:count * sizeof(uint32_t)
                                                   options:MTLResourceStorageModeShared];
            // Output starts poisoned on the CPU; no expected values enter the GPU.
            id<MTLBuffer> output = [device newBufferWithBytes:samples length:count * sizeof(Sample)
                                                    options:MTLResourceStorageModeShared];
            if (!queue || !input || !output)
                return finish(metadata, @"allocation", nil, 3, json, capacity);
            id<MTLCommandBuffer> command = [queue commandBuffer];
            id<MTLComputeCommandEncoder> encoder = [command computeCommandEncoder];
            if (!command || !encoder)
                return finish(metadata, @"command_encoder", nil, 3, json, capacity);
            [encoder setComputePipelineState:pipeline];
            [encoder setBuffer:input offset:0 atIndex:0];
            [encoder setBuffer:output offset:0 atIndex:1];
            [encoder setBytes:&count length:sizeof(count) atIndex:2];
            NSUInteger width = MIN((NSUInteger)64, pipeline.maxTotalThreadsPerThreadgroup);
            if (width == 0) return finish(metadata, @"threadgroup_width", nil, 3, json, capacity);
            [encoder dispatchThreads:MTLSizeMake(count, 1, 1)
               threadsPerThreadgroup:MTLSizeMake(width, 1, 1)];
            [encoder endEncoding];
            [command commit];
            info[1] = 1;
            [command waitUntilCompleted];
            info[2] = (uint32_t)command.status;
            metadata[@"wait_until_completed_called"] = @YES;
            metadata[@"command_status"] = @(command.status);
            if (command.status != MTLCommandBufferStatusCompleted || command.error)
                return finish(metadata, @"command_completion", command.error, 3, json, capacity);
            memcpy(samples, output.contents, count * sizeof(Sample));
            info[3] = 1;
            metadata[@"readback_copied_after_successful_wait"] = @YES;
            metadata[@"availability"] = @"available_and_executed";
            return finish(metadata, @"completed", nil, 0, json, capacity);
        } @catch (NSException *exception) {
            (void)exception;
            return finish(metadata, @"objective_c_exception", nil, 3, json, capacity);
        }
    }
}
