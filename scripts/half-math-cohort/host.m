#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#if !defined(__aarch64__)
#error This probe requires native Apple Silicon.
#endif

typedef struct {
    uint32_t operation, tag, reserved0, reserved1;
    uint32_t a[4], b[4], c[4];
} Input;
typedef struct {
    uint32_t operation, index, tag, stamp;
    uint32_t a[4], b[4], c[4], result[4];
} Sample;
_Static_assert(sizeof(Input) == 64, "input stride");
_Static_assert(sizeof(Sample) == 80, "sample stride");
_Static_assert(offsetof(Sample, result) == 64, "result offset");

static NSNumber *boolean(BOOL value) {
    // Deliberately serialize JSON booleans, never integer 0/1 stand-ins.
    return [NSNumber numberWithBool:value];
}

static int finish(NSMutableDictionary *metadata, NSString *directory,
                  NSString *phase, NSError *error, int status) {
    metadata[@"phase"] = phase;
    metadata[@"exit_code"] = @(status);
    // Never serialize paths, usernames, environment or localized diagnostics.
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
        metadata[@"schema"] = @1;
        metadata[@"api"] = @"Metal compute";
        metadata[@"cohort"] = @"half-math";
        metadata[@"architecture"] = @"arm64";
        metadata[@"availability"] = @"not_executed";
        metadata[@"os_version_and_build"] = NSProcessInfo.processInfo.operatingSystemVersionString;
        metadata[@"os_major"] = @(NSProcessInfo.processInfo.operatingSystemVersion.majorVersion);
        metadata[@"no_cpu_fallback"] = boolean(YES);
        @try {
            NSData *data = [NSData dataWithContentsOfFile:inputPath];
            if (!data || !data.length || data.length % sizeof(Input)
                || data.length / sizeof(Input) > 32768)
                return finish(metadata, directory, @"invalid_input_size", nil, 3);
            uint32_t count = (uint32_t)(data.length / sizeof(Input));
            uint32_t operationCounts[5] = {0};
            for (uint32_t index = 0; index < count; ++index) {
                Input row;
                memcpy(&row, (const uint8_t *)data.bytes + index * sizeof(Input), sizeof(row));
                if (row.operation >= 5 || row.reserved0 || row.reserved1)
                    return finish(metadata, directory, @"invalid_input_record", nil, 3);
                for (uint32_t lane = 0; lane < 4; ++lane)
                    if ((row.a[lane] | row.b[lane] | row.c[lane]) > 0xffffu)
                        return finish(metadata, directory, @"invalid_half_input_bits", nil, 3);
                operationCounts[row.operation] += 1;
            }
            for (uint32_t operation = 0; operation < 5; ++operation)
                if (!operationCounts[operation]) return finish(metadata, directory, @"missing_operation", nil, 3);
            metadata[@"operation_records"] = @[@(operationCounts[0]), @(operationCounts[1]),
                @(operationCounts[2]), @(operationCounts[3]), @(operationCounts[4])];
            metadata[@"threads"] = @(count);
            metadata[@"input_bytes"] = @(data.length);
            metadata[@"result_bytes"] = @(count * sizeof(Sample));
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (!device) return finish(metadata, directory, @"no_metal_device", nil, 2);
            metadata[@"device_name"] = device.name ?: @"unknown";
            metadata[@"device_created"] = boolean(YES);
            metadata[@"unified_memory"] = boolean(device.hasUnifiedMemory);
            if (!device.hasUnifiedMemory) return finish(metadata, directory, @"requires_unified_memory", nil, 2);
            NSError *error = nil;
            id<MTLLibrary> library = [device newLibraryWithURL:[NSURL fileURLWithPath:libraryPath] error:&error];
            if (!library) return finish(metadata, directory, @"load_metallib", error, 3);
            id<MTLFunction> function = [library newFunctionWithName:@"half_math_probe"];
            if (!function) return finish(metadata, directory, @"missing_entry", nil, 3);
            id<MTLComputePipelineState> pipeline = [device newComputePipelineStateWithFunction:function error:&error];
            if (!pipeline) return finish(metadata, directory, @"compute_pipeline", error, 3);
            metadata[@"pipeline_created"] = boolean(YES);
            id<MTLCommandQueue> queue = [device newCommandQueue];
            id<MTLBuffer> input = [device newBufferWithBytes:data.bytes length:data.length options:MTLResourceStorageModeShared];
            NSUInteger outputLength = (count + 1) * sizeof(Sample);
            id<MTLBuffer> output = [device newBufferWithLength:outputLength options:MTLResourceStorageModeShared];
            if (!queue || !input || !output) return finish(metadata, directory, @"allocation", nil, 3);
            metadata[@"buffers_created"] = boolean(YES);
            BOOL uploadedExact = memcmp(input.contents, data.bytes, data.length) == 0;
            metadata[@"input_uploaded_exact"] = boolean(uploadedExact);
            NSData *uploaded = [NSData dataWithBytes:input.contents length:data.length];
            if (![uploaded writeToFile:[directory stringByAppendingPathComponent:@"uploaded-inputs.bin"] atomically:YES]
                || !uploadedExact) return finish(metadata, directory, @"input_upload", nil, 3);
            // Poison every output and one trailing guard record; no expected outputs are uploaded.
            memset(output.contents, 0xa5, outputLength);
            metadata[@"output_poisoned_before_dispatch"] = boolean(YES);
            id<MTLCommandBuffer> command = [queue commandBuffer];
            if (!command) return finish(metadata, directory, @"command_buffer", nil, 3);
            id<MTLComputeCommandEncoder> encoder = [command computeCommandEncoder];
            if (!encoder) return finish(metadata, directory, @"command_encoder", nil, 3);
            [encoder setComputePipelineState:pipeline];
            [encoder setBuffer:input offset:0 atIndex:0];
            [encoder setBuffer:output offset:0 atIndex:1];
            [encoder setBytes:&count length:sizeof(count) atIndex:2];
            NSUInteger width = MIN((NSUInteger)64, pipeline.maxTotalThreadsPerThreadgroup);
            if (!width) return finish(metadata, directory, @"threadgroup_width", nil, 3);
            [encoder dispatchThreads:MTLSizeMake(count, 1, 1) threadsPerThreadgroup:MTLSizeMake(width, 1, 1)];
            [encoder endEncoding];
            metadata[@"dispatch_encoded"] = boolean(YES);
            [command commit];
            metadata[@"command_committed"] = boolean(YES);
            [command waitUntilCompleted];
            metadata[@"wait_until_completed_called"] = boolean(YES);
            metadata[@"command_status"] = @(command.status);
            if (command.status != MTLCommandBufferStatusCompleted || command.error)
                return finish(metadata, directory, @"command_completion", command.error, 3);
            metadata[@"command_completed"] = boolean(YES);
            NSData *observed = [NSData dataWithBytes:output.contents length:count * sizeof(Sample)];
            if (![observed writeToFile:[directory stringByAppendingPathComponent:@"results.bin"] atomically:YES])
                return finish(metadata, directory, @"readback_write", nil, 3);
            metadata[@"readback_copied_after_completion"] = boolean(YES);
            BOOL guardUnchanged = YES;
            const uint8_t *guard = (const uint8_t *)output.contents + count * sizeof(Sample);
            for (NSUInteger index = 0; index < sizeof(Sample); ++index)
                if (guard[index] != 0xa5) guardUnchanged = NO;
            BOOL inputUnchanged = memcmp(input.contents, data.bytes, data.length) == 0;
            metadata[@"output_guard_unchanged"] = boolean(guardUnchanged);
            metadata[@"input_unchanged_after_completion"] = boolean(inputUnchanged);
            if (!guardUnchanged || !inputUnchanged) return finish(metadata, directory, @"buffer_integrity", nil, 3);
            metadata[@"availability"] = @"available_and_executed";
            return finish(metadata, directory, @"completed", nil, 0);
        } @catch (NSException *exception) {
            (void)exception;
            return finish(metadata, directory, @"objective_c_exception", nil, 3);
        }
    }
}
