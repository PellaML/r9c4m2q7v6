#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <dispatch/dispatch.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

// Adapted from an existing validation-only native compute bridge. No CPU math.
// Arguments: source.metal inputs.bin NEW_DIR false|true|default build-request(0|1)
static NSString *escaped(NSString *s) {
    s = [s stringByReplacingOccurrencesOfString:@"\\" withString:@"\\\\"];
    s = [s stringByReplacingOccurrencesOfString:@"\t" withString:@"\\t"];
    s = [s stringByReplacingOccurrencesOfString:@"\r" withString:@"\\r"];
    return [s stringByReplacingOccurrencesOfString:@"\n" withString:@"\\n"];
}
static void field(FILE *f, NSString *key, id value) {
    fprintf(f, "%s\t%s\n", key.UTF8String, escaped([value description]).UTF8String);
    fflush(f);
}
static void failure(FILE *f, const char *phase, NSError *error) {
    field(f, @"failure_phase", @(phase));
    // No descriptions/userInfo: compiler diagnostics can contain local paths.
    field(f, @"failure_domain", error ? error.domain : @"none");
    field(f, @"failure_code", error ? @(error.code) : @0);
}
static id option(MTLCompileOptions *o, NSString *name) {
    return [o respondsToSelector:NSSelectorFromString(name)] ? [o valueForKey:name] : @"unavailable";
}
static BOOL save(NSData *data, NSString *dir, NSString *name) {
    return [data writeToFile:[dir stringByAppendingPathComponent:name] options:0 error:NULL];
}

int main(int argc, const char *argv[]) {
    if (argc != 6 || (strcmp(argv[5], "0") && strcmp(argv[5], "1"))) return 64;
    alarm(60); // bounded process, including runtime compilation; no retry/fallback
    @autoreleasepool {
        NSString *profile = @(argv[4]);
        if (![@[@"false", @"true", @"default"] containsObject:profile]) return 64;
        NSString *dir = @(argv[3]);
        NSFileManager *fm = NSFileManager.defaultManager;
        if ([fm fileExistsAtPath:dir] || ![fm createDirectoryAtPath:dir
            withIntermediateDirectories:NO attributes:nil error:NULL]) return 64;
        FILE *meta = fopen([[dir stringByAppendingPathComponent:@"metadata.tsv"] fileSystemRepresentation], "wx");
        if (!meta) return 70;
        field(meta, @"schema", @"native-dot-v1");
        field(meta, @"backend", @"native_metal");
        field(meta, @"cpu_fallback", @0);
        field(meta, @"profile", profile);
        field(meta, @"build_request", @(argv[5]));
        field(meta, @"os_version_and_build", NSProcessInfo.processInfo.operatingSystemVersionString);
        field(meta, @"host_pointer_bits", @(8 * sizeof(void *)));
        @try {
            NSData *sourceBytes = [NSData dataWithContentsOfFile:@(argv[1])];
            NSData *input = [NSData dataWithContentsOfFile:@(argv[2])];
            if (!sourceBytes || sourceBytes.length > 32768 || !input || input.length == 0 ||
                input.length % 32 || input.length > 32 * 32) {
                failure(meta, "input_bounds", nil); return 64;
            }
            NSString *source = [[NSString alloc] initWithData:sourceBytes encoding:NSUTF8StringEncoding];
            if (!source || !save(sourceBytes, dir, @"loaded-source.metal") ||
                !save(input, dir, @"loaded-input.bin")) {
                failure(meta, "input_record", nil); return 70;
            }
            uint32_t count = (uint32_t)(input.length / 32);
            field(meta, @"case_count", @(count));
            field(meta, @"source_bytes", @(sourceBytes.length));
            field(meta, @"selection", @"system_default_only");
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (!device) { failure(meta, "no_mtl_device_no_fallback", nil); return 23; }
            field(meta, @"device_name", device.name);
            field(meta, @"registry_id", [NSString stringWithFormat:@"%016llx", (unsigned long long)device.registryID]);
            field(meta, @"unified_memory", @(device.hasUnifiedMemory));
            field(meta, @"low_power", @(device.isLowPower));
            field(meta, @"removable", @(device.isRemovable));
            field(meta, @"headless", @(device.isHeadless));
            field(meta, @"max_threadgroup_memory", @(device.maxThreadgroupMemoryLength));
            field(meta, @"max_buffer_length", @(device.maxBufferLength));
            field(meta, @"max_threads_x", @(device.maxThreadsPerThreadgroup.width));
            field(meta, @"max_threads_y", @(device.maxThreadsPerThreadgroup.height));
            field(meta, @"max_threads_z", @(device.maxThreadsPerThreadgroup.depth));
            NSBundle *bundle = [NSBundle bundleForClass:MTLCompileOptions.class];
            field(meta, @"metal_bundle_id", bundle.bundleIdentifier ?: @"unavailable");
            field(meta, @"metal_bundle_version", [bundle objectForInfoDictionaryKey:@"CFBundleVersion"] ?: @"unavailable");
            MTLCompileOptions *options = [MTLCompileOptions new];
            field(meta, @"fast_math_before", @(options.fastMathEnabled));
            BOOL set = ![profile isEqualToString:@"default"];
            if (set) options.fastMathEnabled = [profile isEqualToString:@"true"];
            field(meta, @"fast_math_setter_called", @(set));
            field(meta, @"fast_math_after", @(options.fastMathEnabled));
            field(meta, @"language_version_getter", @(options.languageVersion));
            field(meta, @"math_mode_getter", option(options, @"mathMode"));
            field(meta, @"math_functions_getter", option(options, @"mathFloatingPointFunctions"));
            field(meta, @"other_math_setters_called", @0);
            field(meta, @"compiler_route", @"newLibraryWithSource");
            if (set && options.fastMathEnabled != [profile isEqualToString:@"true"]) {
                failure(meta, "profile_not_retained", nil); return 24;
            }
            NSError *error = nil;
            id<MTLLibrary> library = [device newLibraryWithSource:source options:options error:&error];
            if (!library || error) { failure(meta, "compile", error); return 24; }
            field(meta, @"library_type", @(library.type));
            id<MTLCommandQueue> queue = [device newCommandQueue];
            const NSUInteger guard = 64, body = count * 32 * 4, total = body + guard * 2;
            id<MTLBuffer> values = [device newBufferWithBytes:input.bytes length:input.length
                options:MTLResourceStorageModeShared];
            id<MTLBuffer> result = [device newBufferWithLength:total options:MTLResourceStorageModeShared];
            if (!queue || !values || !result) { failure(meta, "allocation", nil); return 26; }
            field(meta, @"buffer_storage", @"shared");
            field(meta, @"guard_bytes_each_side", @(guard));
            field(meta, @"row_words", @32);
            FILE *commands = fopen([[dir stringByAppendingPathComponent:@"commands.tsv"] fileSystemRepresentation], "wx");
            if (!commands) { failure(meta, "command_record", nil); return 70; }
            fprintf(commands, "kernel\trepeat\tstatus\tno_error\twait_called\tinputs_unchanged\tbytes\tthread_width\tmax_threads\tgpu_start\tgpu_end\n");
            NSArray<NSString *> *names = @[@"dot2", @"dot3", @"dot4", @"add2", @"add3", @"add4", @"noop", @"compose_trace", @"compose_sink"];
            unsigned completed = 0;
            for (NSString *name in names) {
                id<MTLFunction> function = [library newFunctionWithName:name];
                error = nil;
                id<MTLComputePipelineState> pipeline = function ?
                    [device newComputePipelineStateWithFunction:function error:&error] : nil;
                if (!pipeline || error) { failure(meta, "pipeline", error); return 25; }
                for (unsigned repeat = 0; repeat < 3; ++repeat) {
                    // Reset EVERY byte before EVERY dispatch, including the no-op.
                    memset(result.contents, 0xa5, total);
                    id<MTLCommandBuffer> command = [queue commandBuffer];
                    id<MTLComputeCommandEncoder> encoder = [command computeCommandEncoder];
                    if (!command || !encoder) { failure(meta, "encoder", nil); return 26; }
                    [encoder setComputePipelineState:pipeline];
                    [encoder setBuffer:values offset:0 atIndex:0];
                    [encoder setBuffer:result offset:guard atIndex:1];
                    [encoder setBytes:&count length:sizeof(count) atIndex:2];
                    NSUInteger width = MIN((NSUInteger)32, pipeline.maxTotalThreadsPerThreadgroup);
                    if (!width) { failure(meta, "threadgroup", nil); return 26; }
                    [encoder dispatchThreadgroups:MTLSizeMake((count + width - 1) / width, 1, 1)
                        threadsPerThreadgroup:MTLSizeMake(width, 1, 1)];
                    [encoder endEncoding];
                    dispatch_semaphore_t done = dispatch_semaphore_create(0);
                    [command addCompletedHandler:^(id<MTLCommandBuffer> ignored) {
                        (void)ignored; dispatch_semaphore_signal(done);
                    }];
                    [command commit];
                    if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC))) {
                        failure(meta, "gpu_timeout", nil); return 28;
                    }
                    [command waitUntilCompleted];
                    BOOL same = !memcmp(values.contents, input.bytes, input.length);
                    fprintf(commands, "%s\t%u\t%lu\t%d\t1\t%d\t%lu\t%lu\t%lu\t%.17g\t%.17g\n",
                        name.UTF8String, repeat, (unsigned long)command.status, command.error == nil,
                        same, (unsigned long)total, (unsigned long)pipeline.threadExecutionWidth,
                        (unsigned long)pipeline.maxTotalThreadsPerThreadgroup, command.GPUStartTime, command.GPUEndTime);
                    fflush(commands);
                    if (command.status != MTLCommandBufferStatusCompleted || command.error || !same) {
                        failure(meta, "command_or_input_mutation", command.error); return 27;
                    }
                    NSString *raw = [NSString stringWithFormat:@"%@-%u.bin", name, repeat];
                    if (!save([NSData dataWithBytes:result.contents length:total], dir, raw)) {
                        failure(meta, "readback_record", nil); return 70;
                    }
                    ++completed;
                }
            }
            fclose(commands);
            field(meta, @"completed_commands", @(completed));
            field(meta, @"status", @"native_bytes_observed_not_model_qualification");
            fclose(meta);
            return 0;
        } @catch (NSException *exception) {
            field(meta, @"exception_name", exception.name ?: @"unknown");
            failure(meta, "objective_c_exception", nil);
            return 29;
        }
    }
}
