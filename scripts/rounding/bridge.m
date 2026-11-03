#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <objc/runtime.h>
#include <stdint.h>
#include <string.h>

// Private selector surface only. Deliberately no guessed MTLRoundingMode enum.
// The runtime ABI is checked before either scalar method is called.
@interface MTLComputePipelineDescriptor (GenericRoundingProbe)
- (NSInteger)textureWriteRoundingMode;
- (void)setTextureWriteRoundingMode:(NSInteger)mode;
@end

@protocol GenericRoundingSupport
- (BOOL)supportsTextureWriteRoundingMode:(NSInteger)mode;
@end

static NSString *description(id object) {
    NSString *text = [object description] ?: @"";
    // Retain framework-provided names/numbers, not process-specific addresses.
    return [text stringByReplacingOccurrencesOfString:@"0x[0-9a-fA-F]{6,}"
        withString:@"<address>" options:NSRegularExpressionSearch
        range:NSMakeRange(0, text.length)];
}

static int finish(NSMutableDictionary *m, NSString *status, NSError *error,
                  int code, char *json, size_t capacity) {
    m[@"status"] = status;
    m[@"error"] = error ? @{ @"domain": error.domain, @"code": @(error.code) }
                         : (id)[NSNull null];
    NSData *data = [NSJSONSerialization dataWithJSONObject:m
        options:NSJSONWritingSortedKeys error:NULL];
    if (!data || data.length >= capacity) return 70;
    memcpy(json, data.bytes, data.length);
    json[data.length] = 0;
    return code;
}

static BOOL signedInteger(const char *type) {
    // Observed getter q16@0:8, setter v24@0:8q16 and capability B24@0:8q16.
    // Preserve the runtime check; do not accept a merely same-width enum ABI.
    return type && strcmp(type, @encode(NSInteger)) == 0;
}

static id<MTLDevice> inventory(NSMutableDictionary *m) {
    NSArray<id<MTLDevice>> *all = MTLCopyAllDevices();
    NSMutableArray *names = [NSMutableArray array];
    for (id<MTLDevice> item in all) [names addObject:item.name ?: @"unknown"];
    m[@"enumerated_devices"] = names;
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    m[@"selection"] = device ? @"system_default" : @"first_enumerated_or_none";
    if (!device) device = all.firstObject;
    m[@"device_present"] = @(device != nil);
    if (device) {
        m[@"device"] = @{ @"name": device.name ?: @"unknown",
                          @"registry_id": @(device.registryID),
                          @"unified_memory": @(device.hasUnifiedMemory),
                          @"low_power": @(device.isLowPower),
                          @"removable": @(device.isRemovable) };
    }
    return device;
}

// stage 0: descriptor ONLY, 1: GPU observation, 2: device inventory ONLY,
// stage 3: one device capability query (isolated so it cannot lose other modes).
// Format indices below are probe-local, NOT numeric MTLPixelFormat values.
// No CPU expected-output buffer or conversion implementation enters this bridge.
int rounding_native(uint32_t stage, uint32_t mode, uint32_t format, uint32_t fast_math,
                    const uint32_t *words, size_t pixels, const char *source,
                    uint8_t *raw, size_t raw_size, uint32_t *echo,
                    uint64_t *observed_device, char *json, size_t capacity) {
    if (observed_device) *observed_device = 0;
    @autoreleasepool {
        NSMutableDictionary *m = [@{ @"stage": @(stage), @"requested_mode": @(mode),
            @"fast_math_requested": @(fast_math != 0),
            @"command_completed": @NO, @"raw_readback_copied": @NO,
            @"os_version_and_build": NSProcessInfo.processInfo.operatingSystemVersionString
        } mutableCopy];
        @try {
            NSBundle *bundle = [NSBundle bundleForClass:MTLComputePipelineDescriptor.class];
            m[@"framework"] = @{ @"bundle_id": bundle.bundleIdentifier ?: @"unknown",
                @"version": [bundle objectForInfoDictionaryKey:@"CFBundleVersion"] ?: @"unknown" };
            if (stage > 3 || mode > 2 || format > 2 || fast_math > 1)
                return finish(m, @"invalid_arguments", nil, 64, json, capacity);
            if (stage == 2) {
                (void)inventory(m);
                return finish(m, @"inventory_only_not_gpu_proof", nil, 0, json, capacity);
            }
            if (stage == 3) {
                id<MTLDevice> device = inventory(m);
                if (!device) return finish(m, @"no_metal_device_capability_not_queried", nil, 23, json, capacity);
                SEL selector = @selector(supportsTextureWriteRoundingMode:);
                BOOL available = [device respondsToSelector:selector];
                m[@"supportsTextureWriteRoundingMode_selector_available"] = @(available);
                if (!available) return finish(m, @"capability_selector_unavailable", nil, 20, json, capacity);
                NSMethodSignature *sig = [(NSObject *)device methodSignatureForSelector:selector];
                Method method = class_getInstanceMethod([(NSObject *)device class], selector);
                m[@"capability_encoding"] = method ? @(method_getTypeEncoding(method)) : @"missing";
                if (!sig || sig.numberOfArguments != 3 || strcmp(sig.methodReturnType, @encode(BOOL)) ||
                    !signedInteger([sig getArgumentTypeAtIndex:2]))
                    return finish(m, @"capability_abi_not_bool_nsinteger", nil, 21, json, capacity);
                m[@"supportsTextureWriteRoundingMode"] = @([(id<GenericRoundingSupport>)device
                    supportsTextureWriteRoundingMode:(NSInteger)mode]);
                // This property is descriptive only; it neither proves execution nor gates
                // the independently attempted compute pipeline/writeback cases.
                return finish(m, @"capability_only_not_gpu_proof", nil, 0, json, capacity);
            }

            // This entire block precedes ALL device/library/command creation.
            MTLComputePipelineDescriptor *descriptor = [MTLComputePipelineDescriptor new];
            if (!descriptor) return finish(m, @"descriptor_unavailable", nil, 20, json, capacity);
            SEL getter = @selector(textureWriteRoundingMode);
            SEL setter = @selector(setTextureWriteRoundingMode:);
            BOOL hasGet = [descriptor respondsToSelector:getter];
            BOOL hasSet = [descriptor respondsToSelector:setter];
            m[@"descriptor_class"] = NSStringFromClass(descriptor.class);
            m[@"getter_available"] = @(hasGet);
            m[@"setter_available"] = @(hasSet);
            m[@"description_default"] = description(descriptor);
            if (!hasGet || !hasSet)
                return finish(m, @"selector_unavailable", nil, 20, json, capacity);
            NSMethodSignature *getSig = [descriptor methodSignatureForSelector:getter];
            NSMethodSignature *setSig = [descriptor methodSignatureForSelector:setter];
            Method getMethod = class_getInstanceMethod(descriptor.class, getter);
            Method setMethod = class_getInstanceMethod(descriptor.class, setter);
            m[@"getter_encoding"] = getMethod ? @(method_getTypeEncoding(getMethod)) : @"missing";
            m[@"setter_encoding"] = setMethod ? @(method_getTypeEncoding(setMethod)) : @"missing";
            if (!getSig || !setSig || getSig.numberOfArguments != 2 || setSig.numberOfArguments != 3 ||
                !signedInteger(getSig.methodReturnType) || strcmp(setSig.methodReturnType, "v") ||
                !signedInteger([setSig getArgumentTypeAtIndex:2]))
                return finish(m, @"selector_abi_not_nsinteger", nil, 21, json, capacity);
            m[@"getter_default"] = @([descriptor textureWriteRoundingMode]);
            [descriptor setTextureWriteRoundingMode:(NSInteger)mode];
            NSInteger observed = [descriptor textureWriteRoundingMode];
            m[@"getter_after_set"] = @(observed);
            m[@"description_after_set"] = description(descriptor);
            m[@"round_trip_equal"] = @(observed == mode);
            if (observed != mode)
                return finish(m, @"mode_not_retained", nil, 22, json, capacity);
            if (stage == 0)
                return finish(m, @"descriptor_only_not_gpu_proof", nil, 0, json, capacity);

            id<MTLDevice> device = inventory(m);
            if (!device) return finish(m, @"no_metal_device", nil, 23, json, capacity);
            if (observed_device) *observed_device = device.registryID;
            if (!words || !raw || !echo || !source || pixels == 0)
                return finish(m, @"invalid_arguments", nil, 64, json, capacity);
            const MTLPixelFormat formats[] = { MTLPixelFormatRGBA8Unorm,
                MTLPixelFormatBGRA8Unorm, MTLPixelFormatRGBA16Float };
            const char *names[] = { "RGBA8Unorm", "BGRA8Unorm", "RGBA16Float" };
            size_t bpp = format == 2 ? 8 : 4;
            if (raw_size != pixels * bpp)
                return finish(m, @"invalid_output_size", nil, 64, json, capacity);
            m[@"pixel_format_name"] = @(names[format]);
            m[@"pixel_format_value"] = @(formats[format]);
            m[@"pixel_count"] = @(pixels);
            m[@"compile_options"] = @"runtime source; see fast_math_requested/getter; SDK default language version";
            NSError *error = nil;
            MTLCompileOptions *options = [MTLCompileOptions new];
            options.fastMathEnabled = fast_math != 0;
            m[@"language_version_getter"] = @(options.languageVersion);
            m[@"fast_math_enabled_getter"] = @(options.fastMathEnabled);
            if (options.fastMathEnabled != (fast_math != 0))
                return finish(m, @"fast_math_option_not_retained", nil, 31, json, capacity);
            id<MTLLibrary> library = [device newLibraryWithSource:@(source) options:options error:&error];
            if (!library) return finish(m, @"shader_compile_failed", error, 24, json, capacity);
            id<MTLFunction> function = [library newFunctionWithName:@"rounding_write"];
            if (!function) return finish(m, @"entry_missing", nil, 24, json, capacity);
            descriptor.computeFunction = function;
            id<MTLComputePipelineState> pipeline = [device newComputePipelineStateWithDescriptor:descriptor
                options:MTLPipelineOptionNone reflection:NULL error:&error];
            m[@"getter_after_pipeline_attempt"] = @([descriptor textureWriteRoundingMode]);
            if (!pipeline) return finish(m, @"pipeline_creation_failed", error, 25, json, capacity);
            if ([descriptor textureWriteRoundingMode] != mode)
                return finish(m, @"mode_changed_during_pipeline_creation", nil, 22, json, capacity);

            MTLTextureDescriptor *td = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:formats[format]
                width:pixels height:1 mipmapped:NO];
            td.storageMode = MTLStorageModePrivate;
            td.usage = MTLTextureUsageShaderWrite;
            id<MTLTexture> texture = [device newTextureWithDescriptor:td];
            if (!texture) return finish(m, @"format_or_texture_allocation_unavailable", nil, 26, json, capacity);
            // A conservative padded row avoids linear-copy alignment assumptions.
            NSUInteger row = (raw_size + 255u) & ~(NSUInteger)255u;
            id<MTLBuffer> poison = [device newBufferWithLength:row options:MTLResourceStorageModeShared];
            id<MTLBuffer> readback = [device newBufferWithLength:row options:MTLResourceStorageModeShared];
            id<MTLBuffer> input = [device newBufferWithBytes:words length:pixels * 16 options:MTLResourceStorageModeShared];
            id<MTLBuffer> echoed = [device newBufferWithLength:pixels * 16 options:MTLResourceStorageModeShared];
            id<MTLCommandQueue> queue = [device newCommandQueue];
            if (!poison || !readback || !input || !echoed || !queue)
                return finish(m, @"buffer_or_queue_allocation_failed", nil, 27, json, capacity);
            memset(poison.contents, 0xa5, row);
            memset(readback.contents, 0x5a, row);
            memset(echoed.contents, 0xcc, pixels * 16);
            id<MTLCommandBuffer> command = [queue commandBuffer];
            if (!command) return finish(m, @"command_buffer_failed", nil, 27, json, capacity);
            id<MTLBlitCommandEncoder> clear = [command blitCommandEncoder];
            if (!clear) return finish(m, @"clear_encoder_failed", nil, 27, json, capacity);
            [clear copyFromBuffer:poison sourceOffset:0 sourceBytesPerRow:row sourceBytesPerImage:row
                sourceSize:MTLSizeMake(pixels, 1, 1) toTexture:texture destinationSlice:0
                destinationLevel:0 destinationOrigin:MTLOriginMake(0, 0, 0)];
            [clear endEncoding];
            id<MTLComputeCommandEncoder> compute = [command computeCommandEncoder];
            if (!compute) return finish(m, @"compute_encoder_failed", nil, 27, json, capacity);
            [compute setComputePipelineState:pipeline];
            [compute setBuffer:input offset:0 atIndex:0];
            [compute setBuffer:echoed offset:0 atIndex:1];
            [compute setTexture:texture atIndex:0];
            NSUInteger width = MIN((NSUInteger)32, pipeline.maxTotalThreadsPerThreadgroup);
            if (!width) return finish(m, @"threadgroup_width_zero", nil, 27, json, capacity);
            [compute dispatchThreadgroups:MTLSizeMake((pixels + width - 1) / width, 1, 1)
                threadsPerThreadgroup:MTLSizeMake(width, 1, 1)];
            [compute endEncoding];
            id<MTLBlitCommandEncoder> copy = [command blitCommandEncoder];
            if (!copy) return finish(m, @"readback_encoder_failed", nil, 27, json, capacity);
            [copy copyFromTexture:texture sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0, 0, 0)
                sourceSize:MTLSizeMake(pixels, 1, 1) toBuffer:readback destinationOffset:0
                destinationBytesPerRow:row destinationBytesPerImage:row];
            [copy endEncoding];
            [command commit];
            [command waitUntilCompleted];
            m[@"command_status"] = @(command.status);
            m[@"wait_until_completed_called"] = @YES;
            if (command.status != MTLCommandBufferStatusCompleted || command.error)
                return finish(m, @"command_failed", command.error, 28, json, capacity);
            m[@"command_completed"] = @YES;
            memcpy(raw, readback.contents, raw_size);
            memcpy(echo, echoed.contents, pixels * 16);
            m[@"raw_readback_copied"] = @YES;
            m[@"raw_layout"] = format == 2 ? @"packed little-endian RGBA binary16; row padding omitted"
                : @"packed native format channel order; row padding omitted";
            return finish(m, @"gpu_readback_observed_not_yet_classified", nil, 0, json, capacity);
        } @catch (NSException *exception) {
            m[@"exception_name"] = exception.name ?: @"unknown";
            // Reasons/userInfo can contain arbitrary paths; deliberately not exported.
            return finish(m, @"objective_c_exception", nil, 29, json, capacity);
        }
    }
}
