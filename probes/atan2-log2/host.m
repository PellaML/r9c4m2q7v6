#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#if !defined(__aarch64__)
#error This probe requires native Apple Silicon.
#endif

typedef struct {
    uint32_t a, b, tag, reserved;
} Input;
typedef struct {
    uint32_t a, b, index, stamp;
    uint32_t result[4];
} Sample;
_Static_assert(sizeof(Input) == 16, "input stride");
_Static_assert(sizeof(Sample) == 32, "sample stride");
_Static_assert(offsetof(Sample, result) == 16, "result offset");

static NSString *named(NSString *directory, NSString *prefix, NSString *suffix) {
    return [directory stringByAppendingPathComponent:[NSString stringWithFormat:@"%@-%@", prefix, suffix]];
}

static NSNumber *boolean(BOOL value) {
    // Deliberately serialize JSON booleans, never integer 0/1 stand-ins.
    return [NSNumber numberWithBool:value];
}

static int finish(NSMutableDictionary *metadata, NSString *directory, NSString *prefix,
                  NSString *phase, NSError *error, int status) {
    metadata[@"phase"] = phase;
    metadata[@"exit_code"] = @(status);
    // Never serialize paths, usernames, environment or localized diagnostics.
    metadata[@"error"] = error ? @{ @"domain": error.domain, @"code": @(error.code) }
                                : (id)[NSNull null];
    NSData *data = [NSJSONSerialization dataWithJSONObject:metadata
                    options:NSJSONWritingSortedKeys error:NULL];
    if (!data || ![data writeToFile:named(directory, prefix, @"native.json") atomically:YES]) return 3;
    return status;
}

int main(int argc, const char *argv[]) {
    if (argc != 5) return 64;
    @autoreleasepool {
        NSString *libraryPath = [NSString stringWithUTF8String:argv[1]];
        NSString *inputPath = [NSString stringWithUTF8String:argv[2]];
        NSString *directory = [NSString stringWithUTF8String:argv[3]];
        NSString *prefix = [NSString stringWithUTF8String:argv[4]];
        if (!([prefix isEqualToString:@"legacy"] || [prefix isEqualToString:@"modern"])) return 64;
        NSMutableDictionary *metadata = [NSMutableDictionary dictionary];
        metadata[@"schema"] = @1;
        metadata[@"profile"] = prefix;
        metadata[@"api"] = @"Metal compute";
        metadata[@"architecture"] = @"arm64";
        metadata[@"availability"] = @"not_executed";
        metadata[@"os_version_and_build"] = NSProcessInfo.processInfo.operatingSystemVersionString;
        metadata[@"os_major"] = @(NSProcessInfo.processInfo.operatingSystemVersion.majorVersion);
        metadata[@"no_cpu_fallback"] = boolean(YES);
        @try {
            NSData *data = [NSData dataWithContentsOfFile:inputPath];
            if (!data || !data.length || data.length % sizeof(Input)
                || data.length / sizeof(Input) > 8192)
                return finish(metadata, directory, prefix, @"invalid_input_size", nil, 3);
            uint32_t count = (uint32_t)(data.length / sizeof(Input));
            for (uint32_t index = 0; index < count; ++index) {
                Input row;
                memcpy(&row, (const uint8_t *)data.bytes + index * sizeof(Input), sizeof(row));
                if (row.reserved) return finish(metadata, directory, prefix, @"invalid_input_record", nil, 3);
            }
            metadata[@"threads"] = @(count);
            metadata[@"input_bytes"] = @(data.length);
            metadata[@"result_bytes"] = @(count * sizeof(Sample));
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (!device) return finish(metadata, directory, prefix, @"no_metal_device", nil, 2);
            metadata[@"device_name"] = device.name ?: @"unknown";
            metadata[@"device_created"] = boolean(YES);
            metadata[@"unified_memory"] = boolean(device.hasUnifiedMemory);
            if (!device.hasUnifiedMemory) return finish(metadata, directory, prefix, @"requires_unified_memory", nil, 2);
            NSError *error = nil;
            id<MTLLibrary> library = [device newLibraryWithURL:[NSURL fileURLWithPath:libraryPath] error:&error];
            if (!library) return finish(metadata, directory, prefix, @"load_metallib", error, 3);
            id<MTLFunction> function = [library newFunctionWithName:@"atan2_log2_probe"];
            if (!function) return finish(metadata, directory, prefix, @"missing_entry", nil, 3);
            id<MTLComputePipelineState> pipeline = [device newComputePipelineStateWithFunction:function error:&error];
            if (!pipeline) return finish(metadata, directory, prefix, @"compute_pipeline", error, 3);
            metadata[@"pipeline_created"] = boolean(YES);
            id<MTLCommandQueue> queue = [device newCommandQueue];
            id<MTLBuffer> input = [device newBufferWithBytes:data.bytes length:data.length options:MTLResourceStorageModeShared];
            NSUInteger outputLength = (count + 1) * sizeof(Sample);
            id<MTLBuffer> output = [device newBufferWithLength:outputLength options:MTLResourceStorageModeShared];
            if (!queue || !input || !output) return finish(metadata, directory, prefix, @"allocation", nil, 3);
            metadata[@"buffers_created"] = boolean(YES);
            BOOL uploadedExact = memcmp(input.contents, data.bytes, data.length) == 0;
            metadata[@"input_uploaded_exact"] = boolean(uploadedExact);
            NSData *uploaded = [NSData dataWithBytes:input.contents length:data.length];
            if (![uploaded writeToFile:named(directory, prefix, @"uploaded-inputs.bin") atomically:YES]
                || !uploadedExact) return finish(metadata, directory, prefix, @"input_upload", nil, 3);
            // Poison every output and one trailing guard record; no expected outputs are uploaded.
            memset(output.contents, 0xa5, outputLength);
            metadata[@"output_poisoned_before_dispatch"] = boolean(YES);
            id<MTLCommandBuffer> command = [queue commandBuffer];
            if (!command) return finish(metadata, directory, prefix, @"command_buffer", nil, 3);
            id<MTLComputeCommandEncoder> encoder = [command computeCommandEncoder];
            if (!encoder) return finish(metadata, directory, prefix, @"command_encoder", nil, 3);
            [encoder setComputePipelineState:pipeline];
            [encoder setBuffer:input offset:0 atIndex:0];
            [encoder setBuffer:output offset:0 atIndex:1];
            [encoder setBytes:&count length:sizeof(count) atIndex:2];
            NSUInteger width = MIN((NSUInteger)64, pipeline.maxTotalThreadsPerThreadgroup);
            if (!width) return finish(metadata, directory, prefix, @"threadgroup_width", nil, 3);
            [encoder dispatchThreads:MTLSizeMake(count, 1, 1) threadsPerThreadgroup:MTLSizeMake(width, 1, 1)];
            [encoder endEncoding];
            metadata[@"dispatch_encoded"] = boolean(YES);
            [command commit];
            metadata[@"command_committed"] = boolean(YES);
            [command waitUntilCompleted];
            metadata[@"wait_until_completed_called"] = boolean(YES);
            metadata[@"command_status"] = @(command.status);
            if (command.status != MTLCommandBufferStatusCompleted || command.error)
                return finish(metadata, directory, prefix, @"command_completion", command.error, 3);
            metadata[@"command_completed"] = boolean(YES);
            NSData *observed = [NSData dataWithBytes:output.contents length:count * sizeof(Sample)];
            if (![observed writeToFile:named(directory, prefix, @"results.bin") atomically:YES])
                return finish(metadata, directory, prefix, @"readback_write", nil, 3);
            metadata[@"readback_copied_after_completion"] = boolean(YES);
            BOOL guardUnchanged = YES;
            const uint8_t *guard = (const uint8_t *)output.contents + count * sizeof(Sample);
            for (NSUInteger index = 0; index < sizeof(Sample); ++index)
                if (guard[index] != 0xa5) guardUnchanged = NO;
            BOOL inputUnchanged = memcmp(input.contents, data.bytes, data.length) == 0;
            metadata[@"output_guard_unchanged"] = boolean(guardUnchanged);
            metadata[@"input_unchanged_after_completion"] = boolean(inputUnchanged);
            if (!guardUnchanged || !inputUnchanged) return finish(metadata, directory, prefix, @"buffer_integrity", nil, 3);
            metadata[@"availability"] = @"available_and_executed";
            return finish(metadata, directory, prefix, @"completed", nil, 0);
        } @catch (NSException *exception) {
            (void)exception;
            return finish(metadata, directory, prefix, @"objective_c_exception", nil, 3);
        }
    }
}
