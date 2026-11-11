// Thin native bridge: upload owned bytes, record descriptors, return untouched GPU bytes.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <objc/runtime.h>
#include <cfloat>
#include <cstdint>
#include <cstring>
#include <stdexcept>
#include <sys/sysctl.h>

static const NSUInteger Guard = 256, Rows = 64, Block = Guard * 2 + Rows * 64;
static void need(bool ok, const char *reason) {
    if (!ok) throw std::runtime_error(reason);
}
static uint32_t bits(float x) {
    uint32_t b;
    memcpy(&b, &x, sizeof(b));
    return b;
}
static void save(NSString *root, NSString *name, NSData *data) {
    need(data && data.length <= 65536, "file_budget");
    NSError *error = nil;
    need([data writeToFile:[root stringByAppendingPathComponent:name]
                  options:NSDataWritingWithoutOverwriting error:&error], "write_failed");
}
static NSData *read(NSString *root, NSString *name, NSUInteger bytes) {
    NSData *data = [NSData dataWithContentsOfFile:[root stringByAppendingPathComponent:name]];
    need(data && data.length == bytes, "input_size");
    return data;
}
static NSData *json(id value) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:value options:NSJSONWritingSortedKeys error:NULL];
    need(data != nil, "json_failed");
    return data;
}
static NSDictionary *options(MTLCompileOptions *o) {
    NSMutableDictionary *d = [@{@"fast_math": @(o.fastMathEnabled), @"language_version": @(o.languageVersion),
        @"optimization_level": @(o.optimizationLevel), @"preserve_invariance": @(o.preserveInvariance)} mutableCopy];
    if (@available(macOS 15.0, *)) {
        d[@"math_mode"] = @(o.mathMode);
        d[@"math_functions"] = @(o.mathFloatingPointFunctions);
    }
    return d;
}
static NSDictionary *average(MTLSamplerDescriptor *d, BOOL requested) {
    SEL get = NSSelectorFromString(@"lodAverage"), set = NSSelectorFromString(@"setLodAverage:");
    BOOL canGet = [d respondsToSelector:get], canSet = [d respondsToSelector:set];
    NSMethodSignature *g = canGet ? [d methodSignatureForSelector:get] : nil;
    NSMethodSignature *s = canSet ? [d methodSignatureForSelector:set] : nil;
    BOOL abi = g && s && g.numberOfArguments == 2 && s.numberOfArguments == 3 &&
        strcmp(g.methodReturnType, @encode(BOOL)) == 0 && strcmp(s.methodReturnType, @encode(void)) == 0 &&
        strcmp([s getArgumentTypeAtIndex:2], @encode(BOOL)) == 0;
    NSMutableDictionary *r = [@{@"setter_available": @(canSet), @"getter_available": @(canGet),
        @"bool_abi": @(abi), @"requested": @(requested), @"before": @2, @"reported": @2} mutableCopy];
    Method gm = class_getInstanceMethod(d.class, get), sm = class_getInstanceMethod(d.class, set);
    r[@"getter_encoding"] = gm ? @(method_getTypeEncoding(gm)) : @"unavailable";
    r[@"setter_encoding"] = sm ? @(method_getTypeEncoding(sm)) : @"unavailable";
    if (!abi) return r; // Unavailable is NOT evidence of either Boolean state.
    NSInvocation *getter = [NSInvocation invocationWithMethodSignature:g];
    getter.target = d;
    getter.selector = get;
    BOOL value = NO;
    [getter invoke];
    [getter getReturnValue:&value];
    r[@"before"] = @(value);
    NSInvocation *setter = [NSInvocation invocationWithMethodSignature:s];
    setter.target = d;
    setter.selector = set;
    [setter setArgument:&requested atIndex:2];
    [setter invoke];
    [getter invoke];
    [getter getReturnValue:&value];
    r[@"reported"] = @(value);
    return r;
}

int main(int argc, const char **argv) {
    if (argc != 3) return 64;
    @autoreleasepool {
        NSString *root = @(argv[2]);
        NSMutableDictionary *env = [@{@"schema": @1, @"status": @"FAILED_NOT_PROOF",
            @"os_version_and_build": NSProcessInfo.processInfo.operatingSystemVersionString,
            @"host_compiler": @(__clang_version__), @"air_flags_extracted": @NO,
            @"gpu_internal_pitch_observed": @NO, @"sampler_state_internal_bits_observed": @NO} mutableCopy];
        NSMutableArray *calls = [NSMutableArray array], *textures = [NSMutableArray array];
        NSMutableData *receipts = [NSMutableData data];
        int result = 3;
        @try {
            try {
                char build[128] = {};
                size_t length = sizeof(build);
                need(sysctlbyname("kern.osversion", build, &length, NULL, 0) == 0 &&
                     length > 1 && length <= sizeof(build), "os_build_unavailable");
                env[@"os_build"] = @(build);
                NSBundle *bundle = [NSBundle bundleForClass:MTLSamplerDescriptor.class];
                env[@"metal_framework"] = @{@"identifier": bundle.bundleIdentifier ?: @"unavailable",
                    @"build": [bundle objectForInfoDictionaryKey:@"CFBundleVersion"] ?: @"unavailable"};
                NSMutableArray *availability = [NSMutableArray array];
                bool both = true;
                for (unsigned value = 0; value < 2; ++value) {
                    MTLSamplerDescriptor *descriptor = [MTLSamplerDescriptor new];
                    descriptor.normalizedCoordinates = NO;
                    descriptor.minFilter = descriptor.magFilter = MTLSamplerMinMagFilterLinear;
                    descriptor.mipFilter = MTLSamplerMipFilterNotMipmapped;
                    NSDictionary *observation = average(descriptor, value != 0);
                    [availability addObject:observation];
                    both = both && [observation[@"bool_abi"] boolValue] &&
                        [observation[@"reported"] unsignedIntValue] == value;
                }
                env[@"lod_average_preflight"] = availability;
                need(both, "lod_average_unavailable_or_readback_failed_not_proof");
                id<MTLDevice> device = MTLCreateSystemDefaultDevice();
                need(device != nil, "no_native_device_no_fallback");
                env[@"device"] = @{@"name": device.name, @"registry_id": @(device.registryID),
                    @"unified_memory": @(device.hasUnifiedMemory), @"low_power": @(device.isLowPower),
                    @"removable": @(device.isRemovable), @"mac2": @([device supportsFamily:MTLGPUFamilyMac2])};
                MTLCompileOptions *o = [MTLCompileOptions new];
                env[@"default_compiler_options"] = options(o);
                o.fastMathEnabled = NO;
                o.languageVersion = MTLLanguageVersion2_4;
                env[@"effective_compiler_options"] = options(o);
                need(!o.fastMathEnabled, "fast_math_not_disabled");
                NSString *source = [NSString stringWithContentsOfFile:@(argv[1]) encoding:NSUTF8StringEncoding error:NULL];
                need(source && source.length < 16384, "source_unavailable");
                NSError *error = nil;
                id<MTLLibrary> lib = [device newLibraryWithSource:source options:o error:&error];
                need(lib && !error, "msl_compile_failed");
                id<MTLFunction> cf = [lib newFunctionWithName:@"sample_explicit"];
                need(cf != nil, "compute_entry_missing");
                id<MTLComputePipelineState> cp = [device newComputePipelineStateWithFunction:cf error:&error];
                need(cp && !error && cp.maxTotalThreadsPerThreadgroup >= Rows, "compute_pipeline_failed");
                MTLRenderPipelineDescriptor *pd = [MTLRenderPipelineDescriptor new];
                pd.vertexFunction = [lib newFunctionWithName:@"fullscreen"];
                pd.fragmentFunction = [lib newFunctionWithName:@"sample_implicit"];
                pd.colorAttachments[0].pixelFormat = MTLPixelFormatRGBA8Unorm;
                pd.rasterSampleCount = 1;
                need(pd.vertexFunction && pd.fragmentFunction, "render_entry_missing");
                id<MTLRenderPipelineState> rp = [device newRenderPipelineStateWithDescriptor:pd error:&error];
                need(rp && !error, "render_pipeline_failed");
                id<MTLCommandQueue> queue = [device newCommandQueue];
                MTLTextureDescriptor *sd = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
                    width:8 height:8 mipmapped:NO];
                sd.storageMode = MTLStorageModePrivate;
                sd.usage = MTLTextureUsageRenderTarget;
                id<MTLTexture> sink = [device newTextureWithDescriptor:sd];
                need(queue && sink, "queue_or_sink_failed");
                for (unsigned ti = 0; ti < 2; ++ti) {
                    NSUInteger w = ti == 0 ? 7 : 11, h = ti == 0 ? 5 : 3, pitch = ti == 0 ? 256 : 512;
                    NSUInteger transferSize = 2 * Guard + pitch * h;
                    NSData *input = read(root, [NSString stringWithFormat:@"input%u.bin", ti], transferSize);
                    NSData *coordinates = read(root, [NSString stringWithFormat:@"coords%u.bin", ti], Rows * 16);
                    id<MTLBuffer> upload = [device newBufferWithBytes:input.bytes length:input.length options:MTLResourceStorageModeShared];
                    id<MTLBuffer> back = [device newBufferWithLength:transferSize options:MTLResourceStorageModeShared];
                    id<MTLBuffer> uv = [device newBufferWithBytes:coordinates.bytes length:coordinates.length options:MTLResourceStorageModeShared];
                    MTLTextureDescriptor *td = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA16Float
                        width:w height:h mipmapped:NO];
                    td.storageMode = MTLStorageModePrivate;
                    td.usage = MTLTextureUsageShaderRead;
                    id<MTLTexture> tex = [device newTextureWithDescriptor:td];
                    need(upload && back && uv && tex, "texture_buffers_failed");
                    need(tex.width == w && tex.height == h && tex.depth == 1 && tex.arrayLength == 1 &&
                        tex.mipmapLevelCount == 1 && tex.sampleCount == 1 && tex.textureType == MTLTextureType2D &&
                        tex.pixelFormat == MTLPixelFormatRGBA16Float && tex.storageMode == MTLStorageModePrivate &&
                        tex.usage == MTLTextureUsageShaderRead && tex.buffer == nil, "texture_contract");
                    [textures addObject:@{@"id": @(ti), @"width": @(tex.width), @"height": @(tex.height),
                        @"depth": @(tex.depth), @"mips": @(tex.mipmapLevelCount), @"samples": @(tex.sampleCount),
                        @"array_length": @(tex.arrayLength), @"format": @(tex.pixelFormat), @"type": @(tex.textureType),
                        @"storage": @(tex.storageMode), @"usage": @(tex.usage), @"transfer_row_pitch": @(pitch)}];
                    memset(back.contents, 0xa5, back.length);
                    id<MTLCommandBuffer> transfer = [queue commandBuffer];
                    id<MTLBlitCommandEncoder> blit = [transfer blitCommandEncoder];
                    need(transfer && blit, "transfer_encoder_failed");
                    [blit copyFromBuffer:upload sourceOffset:Guard sourceBytesPerRow:pitch sourceBytesPerImage:pitch * h
                        sourceSize:MTLSizeMake(w, h, 1) toTexture:tex destinationSlice:0 destinationLevel:0 destinationOrigin:MTLOriginMake(0, 0, 0)];
                    [blit copyFromTexture:tex sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0, 0, 0)
                        sourceSize:MTLSizeMake(w, h, 1) toBuffer:back destinationOffset:Guard destinationBytesPerRow:pitch destinationBytesPerImage:pitch * h];
                    [blit endEncoding];
                    [transfer commit];
                    [transfer waitUntilCompleted];
                    NSMutableData *transfers = [NSMutableData dataWithBytes:upload.contents length:upload.length];
                    [transfers appendBytes:back.contents length:back.length];
                    save(root, [NSString stringWithFormat:@"transfer%u.bin", ti], transfers);
                    need(transfer.status == MTLCommandBufferStatusCompleted && !transfer.error, "transfer_command_failed");
                    for (unsigned filter = 0; filter < 2; ++filter) for (unsigned mode = 0; mode < 3; ++mode)
                    for (unsigned avg = 0; avg < 2; ++avg) {
                        unsigned caseID = ((ti * 2 + filter) * 3 + mode) * 2 + avg;
                        MTLSamplerDescriptor *d = [MTLSamplerDescriptor new];
                        d.minFilter = filter ? MTLSamplerMinMagFilterLinear : MTLSamplerMinMagFilterNearest;
                        d.magFilter = d.minFilter;
                        d.mipFilter = MTLSamplerMipFilterNotMipmapped;
                        d.normalizedCoordinates = mode != 0;
                        d.sAddressMode = d.tAddressMode = d.rAddressMode = MTLSamplerAddressModeClampToEdge;
                        d.maxAnisotropy = 1;
                        d.lodMinClamp = 0.0f;
                        d.lodMaxClamp = FLT_MAX;
                        d.compareFunction = MTLCompareFunctionNever;
                        NSDictionary *a = average(d, avg != 0);
                        [calls addObject:@{@"id": @(caseID), @"average_api": a}];
                        need([a[@"bool_abi"] boolValue], "lod_average_unavailable_not_proof");
                        need([a[@"reported"] unsignedIntValue] == avg, "lod_average_readback_not_proof");
                        id<MTLSamplerState> sampler = [device newSamplerStateWithDescriptor:d];
                        need(sampler != nil, "sampler_creation_failed");
                        need(d.minFilter == d.magFilter && d.minFilter == filter && d.mipFilter == 0 &&
                            d.normalizedCoordinates == (mode != 0) && d.sAddressMode == 0 && d.tAddressMode == 0 &&
                            d.rAddressMode == 0 && d.maxAnisotropy == 1 && d.compareFunction == 0 &&
                            bits(d.lodMinClamp) == 0 && bits(d.lodMaxClamp) == 0x7f7fffffu, "sampler_contract");
                        id<MTLBuffer> c = [device newBufferWithLength:Block options:MTLResourceStorageModeShared];
                        id<MTLBuffer> r = [device newBufferWithLength:Block options:MTLResourceStorageModeShared];
                        need(c && r, "output_buffer_failed");
                        memset(c.contents, 0xa5, c.length);
                        memset(r.contents, 0xa5, r.length);
                        uint32_t p[4] = {caseID, mode, 0, 0};
                        id<MTLCommandBuffer> cb = [queue commandBuffer];
                        id<MTLComputeCommandEncoder> ce = [cb computeCommandEncoder];
                        need(cb && ce, "compute_encoder_failed");
                        [ce setComputePipelineState:cp];
                        [ce setTexture:tex atIndex:0];
                        [ce setTexture:tex atIndex:1];
                        [ce setSamplerState:sampler atIndex:0];
                        [ce setBuffer:uv offset:0 atIndex:0];
                        [ce setBuffer:c offset:Guard atIndex:1];
                        [ce setBytes:p length:sizeof(p) atIndex:2];
                        [ce dispatchThreadgroups:MTLSizeMake(1, 1, 1) threadsPerThreadgroup:MTLSizeMake(Rows, 1, 1)];
                        [ce endEncoding];
                        MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
                        pass.colorAttachments[0].texture = sink;
                        pass.colorAttachments[0].loadAction = MTLLoadActionDontCare;
                        pass.colorAttachments[0].storeAction = MTLStoreActionDontCare;
                        id<MTLRenderCommandEncoder> re = [cb renderCommandEncoderWithDescriptor:pass];
                        need(re != nil, "render_encoder_failed");
                        [re setRenderPipelineState:rp];
                        [re setViewport:(MTLViewport){0, 0, 8, 8, 0, 1}];
                        [re setFragmentTexture:tex atIndex:0];
                        [re setFragmentTexture:tex atIndex:1];
                        [re setFragmentSamplerState:sampler atIndex:0];
                        [re setFragmentBuffer:uv offset:0 atIndex:0];
                        [re setFragmentBuffer:r offset:Guard atIndex:1];
                        [re setFragmentBytes:p length:sizeof(p) atIndex:2];
                        [re drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
                        [re endEncoding];
                        [cb commit];
                        [cb waitUntilCompleted];
                        NSMutableData *raw = [NSMutableData dataWithBytes:c.contents length:c.length];
                        [raw appendBytes:r.contents length:r.length];
                        save(root, [NSString stringWithFormat:@"case%02u.bin", caseID], raw);
                        NSDictionary *receipt = @{@"id": @(caseID), @"texture": @(ti), @"filter": @(filter), @"mode": @(mode),
                            @"avg": @(avg), @"reported": a[@"reported"], @"setter": a[@"setter_available"],
                            @"getter": a[@"getter_available"], @"abi": a[@"bool_abi"], @"min": @(d.minFilter),
                            @"mag": @(d.magFilter), @"mip": @(d.mipFilter), @"normalized": @(d.normalizedCoordinates),
                            @"s": @(d.sAddressMode), @"t": @(d.tAddressMode), @"r": @(d.rAddressMode),
                            @"anisotropy": @(d.maxAnisotropy), @"compare": @(d.compareFunction),
                            @"lod_min_bits": @(bits(d.lodMinClamp)), @"lod_max_bits": @(bits(d.lodMaxClamp)),
                            @"width": @(tex.width), @"height": @(tex.height), @"mips": @(tex.mipmapLevelCount),
                            @"samples": @(tex.sampleCount), @"status": @(cb.status), @"error": @(cb.error != nil),
                            @"commit": @1, @"wait": @1, @"bytes": @(raw.length)};
                        [receipts appendData:json(receipt)];
                        [receipts appendBytes:"\n" length:1];
                        need(cb.status == MTLCommandBufferStatusCompleted && !cb.error, "sample_command_failed");
                    }
                }
                env[@"status"] = @"CAPTURED_NOT_YET_CHECKED";
                save(root, @"completion.json", json(@{@"cases": @24, @"transfers": @2, @"status": @1}));
                result = 0;
            } catch (const std::exception &e) {
                env[@"failure"] = @(e.what()); // Only our constant reason tokens; no driver log/path text.
            }
        } @catch (NSException *exception) {
            (void)exception;
            env[@"failure"] = @"objective_c_exception_not_proof";
        }
        env[@"sampler_calls"] = calls;
        env[@"textures"] = textures;
        try {
            save(root, @"cases.jsonl", receipts);
            save(root, @"environment.json", json(env));
        } catch (...) { return 70; }
        return result;
    }
}
