#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdint.h>
#include <string.h>

typedef struct {
    uint32_t abi_version, command_status, completed, no_error;
    uint32_t readback_copied, fast_math_observed;
    uint64_t device_registry_id;
} UnaryEvidence;
_Static_assert(sizeof(UnaryEvidence) == 32, "evidence ABI");

static int finish(NSMutableDictionary *m, NSString *status, NSError *error,
                  int code, char *json, size_t capacity) {
    m[@"status"] = status;
    // Do not export descriptions/userInfo: they can contain local paths.
    m[@"error"] = error ? @{ @"domain": error.domain, @"code": @(error.code) }
                         : (id)[NSNull null];
    NSData *data = [NSJSONSerialization dataWithJSONObject:m
        options:NSJSONWritingSortedKeys error:NULL];
    if (!data || data.length >= capacity) return 70;
    memcpy(json, data.bytes, data.length);
    json[data.length] = 0;
    return code;
}

// No CPU arithmetic/reference is available to this bridge.
int unary_native(const uint32_t *input, size_t count, const char *source,
                 uint32_t fast_math, uint32_t variant, uint32_t *output,
                 size_t output_words, UnaryEvidence *e, char *json, size_t capacity) {
    if (!e || !json || !capacity) return 64;
    memset(e, 0, sizeof(*e));
    e->abi_version = 1;
    @autoreleasepool {
        NSMutableDictionary *m = [@{
            @"schema": @1, @"backend": @"native_metal", @"variant": @(variant),
            @"fast_math_requested": @(fast_math != 0), @"command_completed": @NO,
            @"readback_copied": @NO, @"wait_until_completed_called": @NO,
            @"os_version_and_build": NSProcessInfo.processInfo.operatingSystemVersionString
        } mutableCopy];
        @try {
            if (!input || !output || !source || count == 0 || count > 4096 ||
                output_words != count * 3 || fast_math > 1 || variant > 2)
                return finish(m, @"invalid_arguments", nil, 64, json, capacity);
            NSBundle *bundle = [NSBundle bundleForClass:MTLCompileOptions.class];
            m[@"metal_framework"] = @{
                @"bundle_id": bundle.bundleIdentifier ?: @"unknown",
                @"version": [bundle objectForInfoDictionaryKey:@"CFBundleVersion"] ?: @"unknown"
            };
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            m[@"selection"] = @"system_default_only";
            if (!device) return finish(m, @"no_metal_device_no_fallback", nil, 23, json, capacity);
            e->device_registry_id = device.registryID;
            m[@"device"] = @{
                @"name": device.name ?: @"unknown", @"registry_id": @(device.registryID),
                @"unified_memory": @(device.hasUnifiedMemory), @"low_power": @(device.isLowPower),
                @"removable": @(device.isRemovable)
            };
            MTLCompileOptions *options = [MTLCompileOptions new];
            options.fastMathEnabled = fast_math != 0;
            options.preprocessorMacros = @{ @"PROBE_VARIANT": @(variant) };
            e->fast_math_observed = options.fastMathEnabled ? 1 : 0;
            m[@"fast_math_enabled_getter"] = @(options.fastMathEnabled);
            m[@"language_version_getter"] = @(options.languageVersion);
            m[@"compiler_route"] = @"newLibraryWithSource; explicit fast::sin and fast::cos in both math modes";
            if (e->fast_math_observed != fast_math)
                return finish(m, @"fast_math_not_retained", nil, 24, json, capacity);
            NSError *error = nil;
            id<MTLLibrary> library = [device newLibraryWithSource:@(source) options:options error:&error];
            if (!library || error) return finish(m, @"compile_failed_or_reported_error", error, 24, json, capacity);
            id<MTLFunction> function = [library newFunctionWithName:@"unary_probe"];
            if (!function) return finish(m, @"entry_missing", nil, 24, json, capacity);
            error = nil;
            id<MTLComputePipelineState> pipeline = [device newComputePipelineStateWithFunction:function error:&error];
            if (!pipeline || error) return finish(m, @"pipeline_failed_or_reported_error", error, 25, json, capacity);
            id<MTLBuffer> values = [device newBufferWithBytes:input length:count * 4
                options:MTLResourceStorageModeShared];
            id<MTLBuffer> result = [device newBufferWithLength:output_words * 4
                options:MTLResourceStorageModeShared];
            id<MTLCommandQueue> queue = [device newCommandQueue];
            if (!values || !result || !queue)
                return finish(m, @"allocation_failed", nil, 26, json, capacity);
            memset(result.contents, 0xa5, output_words * 4);
            id<MTLCommandBuffer> command = [queue commandBuffer];
            if (!command) return finish(m, @"command_unavailable", nil, 26, json, capacity);
            id<MTLComputeCommandEncoder> encoder = [command computeCommandEncoder];
            if (!encoder) return finish(m, @"encoder_unavailable", nil, 26, json, capacity);
            [encoder setComputePipelineState:pipeline];
            [encoder setBuffer:values offset:0 atIndex:0];
            [encoder setBuffer:result offset:0 atIndex:1];
            uint32_t n = (uint32_t)count;
            [encoder setBytes:&n length:sizeof(n) atIndex:2];
            NSUInteger width = MIN((NSUInteger)64, pipeline.maxTotalThreadsPerThreadgroup);
            if (!width) return finish(m, @"zero_threadgroup_width", nil, 26, json, capacity);
            [encoder dispatchThreadgroups:MTLSizeMake((count + width - 1) / width, 1, 1)
                threadsPerThreadgroup:MTLSizeMake(width, 1, 1)];
            [encoder endEncoding];
            [command commit];
            [command waitUntilCompleted];
            e->command_status = (uint32_t)command.status;
            e->completed = command.status == MTLCommandBufferStatusCompleted;
            e->no_error = command.error == nil;
            m[@"command_status"] = @(command.status);
            m[@"command_status_name"] = e->completed ? @"completed" : @"not_completed";
            m[@"command_completed"] = @(e->completed != 0);
            m[@"wait_until_completed_called"] = @YES;
            m[@"command_error"] = command.error ? @{
                @"domain": command.error.domain, @"code": @(command.error.code)
            } : (id)[NSNull null];
            if (!e->completed || !e->no_error)
                return finish(m, @"command_failed", command.error, 27, json, capacity);
            // Shared storage is read only after successful GPU completion.
            memcpy(output, result.contents, output_words * 4);
            e->readback_copied = 1;
            m[@"readback_copied"] = @YES;
            m[@"output_byte_length"] = @(output_words * 4);
            m[@"layout"] = @"little-endian uint32 triples: echoed_input_bits,sin_bits,cos_bits";
            return finish(m, @"gpu_readback_observed", nil, 0, json, capacity);
        } @catch (NSException *exception) {
            m[@"exception_name"] = exception.name ?: @"unknown";
            return finish(m, @"objective_c_exception", nil, 29, json, capacity);
        }
    }
}
