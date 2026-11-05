#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdint.h>
#include <string.h>

typedef struct {
    uint32_t abi,status,completed,no_error,copied,count,variant,guards;
    uint64_t device_id;
} Evidence;
_Static_assert(sizeof(Evidence)==40,"evidence ABI");
enum { GUARD=256, WIDTH=8 };
static int finish(NSMutableDictionary *m,NSString *state,NSError *error,int code,char *json,size_t cap) {
    m[@"status"]=state;
    m[@"error"]=error?@{@"domain":error.domain,@"code":@(error.code)}:(id)[NSNull null];
    NSData *b=[NSJSONSerialization dataWithJSONObject:m options:NSJSONWritingSortedKeys error:NULL];
    if(!b || b.length>=cap)return 70;
    memcpy(json,b.bytes,b.length);json[b.length]=0;return code;
}
static NSDictionary *options_record(MTLCompileOptions *o) {
    NSMutableDictionary *d=[@{@"fast_math":@(o.fastMathEnabled),@"language_version":@(o.languageVersion),
        @"optimization_level":@(o.optimizationLevel),@"preserve_invariance":@(o.preserveInvariance)} mutableCopy];
    if(@available(macOS 15.0,*)) {d[@"math_mode"]=@(o.mathMode);d[@"math_functions"]=@(o.mathFloatingPointFunctions);}
    return d;
}
static BOOL guards_ok(id<MTLBuffer> b,NSUInteger bytes) {
    const uint8_t *p=b.contents;
    for(NSUInteger i=0;i<b.length;i++)
        if((i<GUARD || i>=GUARD+bytes) && p[i]!=0xa5)return NO;
    return YES;
}
// No CPU half arithmetic or pre-clamp: transfer inputs, encode native work,
// validate completion/guards, and copy the three raw GPU output buffers.
int clamp_native(const uint16_t *input,size_t count,uint32_t variant,const char *source,
        uint16_t *echo,uint16_t *mid,uint16_t *result,Evidence *e,char *json,size_t cap) {
    if(!e || !json || !cap)return 64;
    memset(e,0,sizeof(*e));e->abi=1;
    @autoreleasepool {
        NSMutableDictionary *m=[@{@"schema":@1,@"backend":@"native_metal",@"count":@(count),@"variant":@(variant),
            @"command_completed":@NO,@"readback_copied":@NO,@"os_version_and_build":NSProcessInfo.processInfo.operatingSystemVersionString} mutableCopy];
        @try {
            if(!input || !source || !echo || !mid || !result || (count!=32 && count!=8) || variant>1 || (variant==1 && count!=32))
                return finish(m,@"invalid_arguments",nil,64,json,cap);
            id<MTLDevice> device=MTLCreateSystemDefaultDevice();
            if(!device)return finish(m,@"no_metal_device_no_fallback",nil,23,json,cap);
            e->device_id=device.registryID;e->count=(uint32_t)count;e->variant=variant;
            m[@"device"]=@{@"name":device.name?:@"unknown",@"registry_id":@(device.registryID),
                @"unified_memory":@(device.hasUnifiedMemory),@"mac2":@([device supportsFamily:MTLGPUFamilyMac2]),
                @"apple7":@([device supportsFamily:MTLGPUFamilyApple7]),@"metal3":@([device supportsFamily:MTLGPUFamilyMetal3])};
            MTLCompileOptions *opts=[MTLCompileOptions new];
            m[@"default_compiler_options"]=options_record(opts);
            opts.fastMathEnabled=NO;opts.preprocessorMacros=@{@"WRONG_ENDPOINT":@(variant)};
            m[@"effective_compiler_options"]=options_record(opts);
            if(opts.fastMathEnabled)return finish(m,@"fast_math_not_disabled",nil,24,json,cap);
            NSError *error=nil;
            id<MTLLibrary> library=[device newLibraryWithSource:@(source) options:opts error:&error];
            if(!library || error)return finish(m,@"compile_failed",error,24,json,cap);
            MTLRenderPipelineDescriptor *pd=[MTLRenderPipelineDescriptor new];
            pd.vertexFunction=[library newFunctionWithName:@"clamp_vertex"];
            pd.fragmentFunction=[library newFunctionWithName:@"clamp_fragment"];
            pd.rasterSampleCount=1;pd.colorAttachments[0].pixelFormat=MTLPixelFormatRGBA8Unorm;
            pd.colorAttachments[0].blendingEnabled=NO;pd.colorAttachments[0].writeMask=MTLColorWriteMaskAll;
            pd.depthAttachmentPixelFormat=MTLPixelFormatInvalid;pd.stencilAttachmentPixelFormat=MTLPixelFormatInvalid;
            if(!pd.vertexFunction || !pd.fragmentFunction)return finish(m,@"entry_missing",nil,24,json,cap);
            id<MTLRenderPipelineState> pipeline=[device newRenderPipelineStateWithDescriptor:pd error:&error];
            if(!pipeline || error)return finish(m,@"pipeline_failed",error,25,json,cap);
            MTLTextureDescriptor *td=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
                width:WIDTH height:count/WIDTH mipmapped:NO];
            td.usage=MTLTextureUsageRenderTarget;td.storageMode=MTLStorageModePrivate;
            id<MTLTexture> sink=[device newTextureWithDescriptor:td];
            NSUInteger in_bytes=count*5*sizeof(uint16_t),value_bytes=count*sizeof(uint16_t);
            id<MTLBuffer> inputs=[device newBufferWithBytes:input length:in_bytes options:MTLResourceStorageModeShared];
            id<MTLBuffer> echoes=[device newBufferWithLength:in_bytes+2*GUARD options:MTLResourceStorageModeShared];
            id<MTLBuffer> intermediates=[device newBufferWithLength:value_bytes+2*GUARD options:MTLResourceStorageModeShared];
            id<MTLBuffer> results=[device newBufferWithLength:2*value_bytes+2*GUARD options:MTLResourceStorageModeShared];
            id<MTLCommandQueue> queue=[device newCommandQueue];
            if(!sink || !inputs || !echoes || !intermediates || !results || !queue)
                return finish(m,@"allocation_failed",nil,26,json,cap);
            if(sink.width!=WIDTH || sink.height!=count/WIDTH || sink.pixelFormat!=MTLPixelFormatRGBA8Unorm ||
                sink.usage!=MTLTextureUsageRenderTarget || sink.storageMode!=MTLStorageModePrivate ||
                sink.mipmapLevelCount!=1 || sink.sampleCount!=1 || sink.arrayLength!=1)
                return finish(m,@"sink_properties_invalid",nil,26,json,cap);
            memset(echoes.contents,0xa5,echoes.length);memset(intermediates.contents,0xa5,intermediates.length);memset(results.contents,0xa5,results.length);
            MTLRenderPassDescriptor *pass=[MTLRenderPassDescriptor renderPassDescriptor];
            pass.colorAttachments[0].texture=sink;pass.colorAttachments[0].loadAction=MTLLoadActionDontCare;
            pass.colorAttachments[0].storeAction=MTLStoreActionDontCare;
            id<MTLCommandBuffer> command=[queue commandBuffer];
            if(!command)return finish(m,@"command_unavailable",nil,26,json,cap);
            id<MTLRenderCommandEncoder> re=[command renderCommandEncoderWithDescriptor:pass];
            if(!re)return finish(m,@"encoder_unavailable",nil,26,json,cap);
            [re setRenderPipelineState:pipeline];[re setFragmentBuffer:inputs offset:0 atIndex:0];
            [re setFragmentBuffer:echoes offset:GUARD atIndex:1];[re setFragmentBuffer:intermediates offset:GUARD atIndex:2];
            [re setFragmentBuffer:results offset:GUARD atIndex:3];uint32_t n=(uint32_t)count;[re setFragmentBytes:&n length:sizeof(n) atIndex:4];
            [re drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];[re endEncoding];
            [command commit];[command waitUntilCompleted];
            e->status=(uint32_t)command.status;e->completed=command.status==MTLCommandBufferStatusCompleted;e->no_error=command.error==nil;
            m[@"command_status"]=@(command.status);m[@"command_completed"]=@(e->completed!=0);m[@"wait_until_completed_called"]=@YES;
            if(!e->completed || !e->no_error)return finish(m,@"command_failed",command.error,27,json,cap);
            e->guards=guards_ok(echoes,in_bytes)&&guards_ok(intermediates,value_bytes)&&guards_ok(results,2*value_bytes);
            if(!e->guards)return finish(m,@"output_guard_changed",nil,28,json,cap);
            memcpy(echo,(const uint8_t *)echoes.contents+GUARD,in_bytes);
            memcpy(mid,(const uint8_t *)intermediates.contents+GUARD,value_bytes);
            const uint16_t *r=(const uint16_t *)((const uint8_t *)results.contents+GUARD);
            for(NSUInteger i=0;i<count;i++) {
                if(r[2*i+1]!=(uint16_t)(0xd100u+i))return finish(m,@"output_marker_missing",nil,28,json,cap);
                result[i]=r[2*i];
            }
            m[@"gpu_result_markers_verified"]=@YES;
            e->copied=1;m[@"all_guards_intact"]=@YES;m[@"readback_copied"]=@YES;
            m[@"numeric_route"]=@"fragment half clamp; direct raw integer sidecars; dummy color texture never read";
            m[@"layout"]=@"little-endian uint16; echo five words per case; intermediate/result one word per case";
            return finish(m,@"gpu_readback_observed",nil,0,json,cap);
        } @catch(NSException *exception) {
            m[@"exception_name"]=exception.name?:@"unknown";
            return finish(m,@"objective_c_exception",nil,29,json,cap);
        }
    }
}
