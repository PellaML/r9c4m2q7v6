#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

typedef struct { uint32_t f[2]; uint16_t h[3], reserved; } Input;
typedef struct {
    uint32_t input_f[2], output_f[2];
    uint16_t input_h[3], output_h[3];
    uint32_t index, stamp, reserved;
} Sample;
_Static_assert(sizeof(Input) == 16, "input stride");
_Static_assert(sizeof(Sample) == 40, "sample stride");
_Static_assert(offsetof(Sample, output_h) == 22, "half output offset");
_Static_assert(offsetof(Sample, index) == 28, "index offset");

static int finish(NSMutableDictionary *metadata, NSString *directory,
                  NSString *phase, NSError *error, int status) {
    metadata[@"phase"] = phase;
    // Paths, environment, usernames and localized diagnostics are never serialized.
    metadata[@"error"] = error ? @{ @"domain": error.domain, @"code": @(error.code) }
                                : (id)[NSNull null];
    NSData *data = [NSJSONSerialization dataWithJSONObject:metadata
                    options:NSJSONWritingSortedKeys error:NULL];
    if (!data || ![data writeToFile:[directory stringByAppendingPathComponent:@"native.json"] atomically:YES]) return 3;
    return status;
}

int main(int argc, const char *argv[]) {
    if (argc != 4) return 64;
    @autoreleasepool {
        NSString *libraryPath = [NSString stringWithUTF8String:argv[1]];
        NSString *inputPath = [NSString stringWithUTF8String:argv[2]];
        NSString *directory = [NSString stringWithUTF8String:argv[3]];
        NSMutableDictionary *metadata = [NSMutableDictionary dictionary];
        metadata[@"api"] = @"Metal compute";
        metadata[@"availability"] = @"not_executed";
        metadata[@"os_version_and_build"] = NSProcessInfo.processInfo.operatingSystemVersionString;
        @try {
            NSData *data = [NSData dataWithContentsOfFile:inputPath];
            const uint32_t count = 63488;
            if (!data || data.length != count * sizeof(Input))
                return finish(metadata, directory, @"invalid_input", nil, 3);
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (!device) {
                metadata[@"availability"] = @"unavailable";
                return finish(metadata, directory, @"no_metal_device", nil, 2);
            }
            metadata[@"device_name"] = device.name ?: @"unknown";
            metadata[@"unified_memory"] = @(device.hasUnifiedMemory);
            if (!device.hasUnifiedMemory)
                return finish(metadata, directory, @"requires_unified_memory", nil, 2);
            NSError *error = nil;
            id<MTLLibrary> library = [device newLibraryWithURL:[NSURL fileURLWithPath:libraryPath] error:&error];
            if (!library) return finish(metadata, directory, @"load_metallib", error, 3);
            id<MTLFunction> function = [library newFunctionWithName:@"fract_probe"];
            if (!function) return finish(metadata, directory, @"missing_entry", nil, 3);
            id<MTLComputePipelineState> pipeline = [device newComputePipelineStateWithFunction:function error:&error];
            if (!pipeline) return finish(metadata, directory, @"compute_pipeline", error, 3);
            id<MTLCommandQueue> queue = [device newCommandQueue];
            id<MTLBuffer> input = [device newBufferWithBytes:data.bytes length:data.length options:MTLResourceStorageModeShared];
            id<MTLBuffer> output = [device newBufferWithLength:count * sizeof(Sample) options:MTLResourceStorageModeShared];
            if (!queue || !input || !output) return finish(metadata, directory, @"allocation", nil, 3);
            // No expected result enters the GPU. Every output byte starts poisoned.
            memset(output.contents, 0xa5, count * sizeof(Sample));
            id<MTLCommandBuffer> command = [queue commandBuffer];
            id<MTLComputeCommandEncoder> encoder = [command computeCommandEncoder];
            if (!command || !encoder) return finish(metadata, directory, @"command_encoder", nil, 3);
            [encoder setComputePipelineState:pipeline];
            [encoder setBuffer:input offset:0 atIndex:0];
            [encoder setBuffer:output offset:0 atIndex:1];
            [encoder setBytes:&count length:sizeof(count) atIndex:2];
            NSUInteger width = MIN((NSUInteger)64, pipeline.maxTotalThreadsPerThreadgroup);
            if (!width) return finish(metadata, directory, @"threadgroup_width", nil, 3);
            [encoder dispatchThreads:MTLSizeMake(count, 1, 1) threadsPerThreadgroup:MTLSizeMake(width, 1, 1)];
            [encoder endEncoding];
            [command commit];
            [command waitUntilCompleted];
            metadata[@"pipeline_created"] = @YES;
            metadata[@"wait_until_completed_called"] = @YES;
            metadata[@"command_status"] = @(command.status);
            metadata[@"threads"] = @(count);
            if (command.status != MTLCommandBufferStatusCompleted || command.error)
                return finish(metadata, directory, @"command_completion", command.error, 3);
            NSData *observed = [NSData dataWithBytes:output.contents length:count * sizeof(Sample)];
            if (![observed writeToFile:[directory stringByAppendingPathComponent:@"samples.bin"] atomically:YES])
                return finish(metadata, directory, @"readback_write", nil, 3);
            metadata[@"readback_copied_after_successful_wait"] = @YES;
            metadata[@"availability"] = @"available_and_executed";
            return finish(metadata, directory, @"completed", nil, 0);
        } @catch (NSException *exception) {
            (void)exception;
            return finish(metadata, directory, @"objective_c_exception", nil, 3);
        }
    }
}
