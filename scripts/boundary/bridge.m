#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdint.h>
#include <string.h>

// Only native resource setup, command encoding and completed byte copies.
typedef struct {
    uint32_t abi, status, completed, no_error, copied;
    uint32_t format, route, count, variant, sidecar_stage;
    uint64_t device_id;
} Evidence;
_Static_assert(sizeof(Evidence)==48, "evidence ABI");
_Static_assert(MTLPixelFormatRG16Float==65 && MTLPixelFormatRGBA16Float==115, "SDK format enums");
enum { GUARD=256, STRIDE=256, WIDTH=8 };
static int finish(NSMutableDictionary *m,NSString *status,NSError *err,int code,char *json,size_t cap) {
    m[@"status"]=status;
    m[@"error"]=err?@{@"domain":err.domain,@"code":@(err.code)}:(id)[NSNull null];
    NSData *d=[NSJSONSerialization dataWithJSONObject:m options:NSJSONWritingSortedKeys error:NULL];
    if(!d || d.length>=cap)return 70;
    memcpy(json,d.bytes,d.length);json[d.length]=0;return code;
}
static BOOL edge_guards(id<MTLBuffer> b,NSUInteger payload) {
    const uint8_t *p=b.contents;
    for(NSUInteger i=0;i<b.length;i++)
        if((i<GUARD || i>=GUARD+payload) && p[i]!=0xa5)return NO;
    return YES;
}
static NSDictionary *compiler_settings(MTLCompileOptions *o) {
    NSMutableDictionary *d=[@{@"fast_math":@(o.fastMathEnabled),@"language_version":@(o.languageVersion),
        @"preserve_invariance":@(o.preserveInvariance),@"optimization_level":@(o.optimizationLevel)} mutableCopy];
    if(@available(macOS 15.0,*)) {
        d[@"math_mode"]=@(o.mathMode);
        d[@"math_functions"]=@(o.mathFloatingPointFunctions);
    }
    return d;
}
int boundary_native(uint32_t channels,uint32_t route,uint32_t variant,const char *source,
        const uint32_t *input,size_t count,uint16_t *pixels,uint32_t *echo,uint16_t *casts,
        Evidence *e,char *json,size_t cap) {
    if(!e || !json || !cap)return 64;
    memset(e,0,sizeof(*e));e->abi=1;
    @autoreleasepool {
        NSMutableDictionary *m=[@{@"schema":@1,@"backend":@"native_metal",@"route":@(route),
            @"variant":@(variant),@"count":@(count),@"command_completed":@NO,@"readback_copied":@NO,
            @"os_version_and_build":NSProcessInfo.processInfo.operatingSystemVersionString} mutableCopy];
        @try {
            if((channels!=2 && channels!=4) || route>3 || variant>1 || (count!=8 && count!=64) ||
                !source || !input || !pixels || !echo || !casts)
                return finish(m,@"invalid_arguments",nil,64,json,cap);
            id<MTLDevice> device=MTLCreateSystemDefaultDevice();
            if(!device)return finish(m,@"no_metal_device_no_fallback",nil,23,json,cap);
            e->device_id=device.registryID;e->route=route;e->variant=variant;e->count=(uint32_t)count;
            e->sidecar_stage=route==0?1:2;
            m[@"device"]=@{@"name":device.name?:@"unknown",@"registry_id":@(device.registryID),
                @"unified_memory":@(device.hasUnifiedMemory),@"mac2":@([device supportsFamily:MTLGPUFamilyMac2]),
                @"apple7":@([device supportsFamily:MTLGPUFamilyApple7]),@"metal3":@([device supportsFamily:MTLGPUFamilyMetal3])};
            MTLCompileOptions *opts=[MTLCompileOptions new];
            m[@"default_compiler_options"]=compiler_settings(opts);
            // Keep nonfinite inputs meaningful; do not override language,
            // optimization, or native texture-write rounding behavior.
            opts.fastMathEnabled=NO;
            opts.preprocessorMacros=@{@"WRONG_EXPORT":@(variant)};
            m[@"effective_compiler_options"]=compiler_settings(opts);
            m[@"texture_write_rounding_override"]=@"none; runtime compiler default";
            if(opts.fastMathEnabled)return finish(m,@"math_option_not_retained",nil,24,json,cap);
            NSError *error=nil;
            id<MTLLibrary> library=[device newLibraryWithSource:@(source) options:opts error:&error];
            if(!library || error)return finish(m,@"compile_failed",error,24,json,cap);
            MTLPixelFormat format=channels==2?MTLPixelFormatRG16Float:MTLPixelFormatRGBA16Float;
            e->format=(uint32_t)format;
            id<MTLRenderPipelineState> render=nil;
            id<MTLComputePipelineState> compute=nil;
            if(route==0) {
                id<MTLFunction> f=[library newFunctionWithName:@"boundary_audit"];
                if(!f)return finish(m,@"audit_entry_missing",nil,24,json,cap);
                compute=[device newComputePipelineStateWithFunction:f error:&error];
                if(!compute || error)return finish(m,@"audit_pipeline_failed",error,25,json,cap);
            } else {
                NSArray *names=@[@"unused",@"boundary_float",@"boundary_half",@"boundary_widen"];
                MTLRenderPipelineDescriptor *pd=[MTLRenderPipelineDescriptor new];
                pd.vertexFunction=[library newFunctionWithName:@"boundary_vertex"];
                pd.fragmentFunction=[library newFunctionWithName:names[route]];
                pd.rasterSampleCount=1;
                pd.colorAttachments[0].pixelFormat=format;
                pd.colorAttachments[0].blendingEnabled=NO;
                pd.colorAttachments[0].writeMask=MTLColorWriteMaskAll;
                pd.depthAttachmentPixelFormat=MTLPixelFormatInvalid;
                pd.stencilAttachmentPixelFormat=MTLPixelFormatInvalid;
                if(!pd.vertexFunction || !pd.fragmentFunction)return finish(m,@"render_entry_missing",nil,24,json,cap);
                m[@"pipeline"]=@{@"format":@(pd.colorAttachments[0].pixelFormat),@"blending":@(pd.colorAttachments[0].blendingEnabled),
                    @"write_mask":@(pd.colorAttachments[0].writeMask),@"samples":@(pd.rasterSampleCount),
                    @"depth":@(pd.depthAttachmentPixelFormat),@"stencil":@(pd.stencilAttachmentPixelFormat)};
                render=[device newRenderPipelineStateWithDescriptor:pd error:&error];
                if(!render || error)return finish(m,@"render_pipeline_failed",error,25,json,cap);
            }
            NSUInteger ntextures=route==0?count:1, rows=route==0?count:count/WIDTH;
            NSUInteger payload_per_row=(route==0?1:WIDTH)*channels*2;
            NSMutableArray *textures=[NSMutableArray array];
            MTLTextureDescriptor *td=[MTLTextureDescriptor new];
            td.textureType=MTLTextureType2D;td.pixelFormat=format;
            td.width=route==0?1:WIDTH;td.height=route==0?1:count/WIDTH;
            td.depth=1;td.mipmapLevelCount=1;td.arrayLength=1;td.sampleCount=1;
            td.usage=MTLTextureUsageRenderTarget;td.storageMode=MTLStorageModePrivate;
            for(NSUInteger i=0;i<ntextures;i++) {
                id<MTLTexture> t=[device newTextureWithDescriptor:td];
                if(!t || t.pixelFormat!=format || t.usage!=MTLTextureUsageRenderTarget || t.storageMode!=MTLStorageModePrivate ||
                    t.width!=td.width || t.height!=td.height || t.depth!=1 || t.sampleCount!=1 || t.mipmapLevelCount!=1 ||
                    t.arrayLength!=1 || t.textureType!=MTLTextureType2D || t.framebufferOnly)
                    return finish(m,@"texture_property_failure",nil,26,json,cap);
                [textures addObject:t];
            }
            m[@"textures"]=@{@"count":@(ntextures),@"width":@(td.width),@"height":@(td.height),@"format":@(format),
                @"usage":@(td.usage),@"storage":@(td.storageMode),@"mips":@(td.mipmapLevelCount),@"samples":@(td.sampleCount)};
            m[@"sidecar_stage"]=route==0?@"compute_before_clear":@"fragment_same_invocation";
            id<MTLBuffer> input_buffer=[device newBufferWithBytes:input length:count*16 options:MTLResourceStorageModeShared];
            id<MTLBuffer> eb=[device newBufferWithLength:count*16+2*GUARD options:MTLResourceStorageModeShared];
            id<MTLBuffer> hb=[device newBufferWithLength:count*8+2*GUARD options:MTLResourceStorageModeShared];
            id<MTLBuffer> rb=[device newBufferWithLength:rows*STRIDE+2*GUARD options:MTLResourceStorageModeShared];
            id<MTLCommandQueue> queue=[device newCommandQueue];
            if(!input_buffer || !eb || !hb || !rb || !queue)return finish(m,@"allocation_failed",nil,26,json,cap);
            memset(eb.contents,0xa5,eb.length);memset(hb.contents,0xa5,hb.length);memset(rb.contents,0xa5,rb.length);
            id<MTLCommandBuffer> command=[queue commandBuffer];
            if(!command)return finish(m,@"command_unavailable",nil,26,json,cap);
            if(route==0) {
                id<MTLComputeCommandEncoder> ce=[command computeCommandEncoder];
                if(!ce || compute.maxTotalThreadsPerThreadgroup<count)return finish(m,@"audit_encoder_failure",nil,26,json,cap);
                [ce setComputePipelineState:compute];[ce setBuffer:input_buffer offset:0 atIndex:0];
                [ce setBuffer:eb offset:GUARD atIndex:1];[ce setBuffer:hb offset:GUARD atIndex:2];
                uint32_t n=(uint32_t)count;[ce setBytes:&n length:sizeof(n) atIndex:3];
                [ce dispatchThreadgroups:MTLSizeMake(1,1,1) threadsPerThreadgroup:MTLSizeMake(count,1,1)];
                [ce endEncoding];
            }
            for(NSUInteger i=0;i<ntextures;i++) {
                MTLRenderPassDescriptor *rp=[MTLRenderPassDescriptor renderPassDescriptor];
                rp.colorAttachments[0].texture=textures[i];rp.colorAttachments[0].loadAction=MTLLoadActionClear;
                rp.colorAttachments[0].storeAction=MTLStoreActionStore;
                MTLClearColor color=MTLClearColorMake(-3.0,7.0,-5.0,9.0);
                if(route==0) {
                    float x[4];memcpy(x,input+4*i,sizeof(x));
                    if(variant && i==0)x[0]=-x[0];
                    color=MTLClearColorMake((double)x[0],(double)x[1],(double)x[2],(double)x[3]);
                }
                rp.colorAttachments[0].clearColor=color;
                id<MTLRenderCommandEncoder> re=[command renderCommandEncoderWithDescriptor:rp];
                if(!re)return finish(m,@"render_encoder_failed",nil,26,json,cap);
                if(route!=0) {
                    [re setRenderPipelineState:render];[re setFragmentBuffer:input_buffer offset:0 atIndex:0];
                    [re setFragmentBuffer:eb offset:GUARD atIndex:1];[re setFragmentBuffer:hb offset:GUARD atIndex:2];
                    [re drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
                }
                [re endEncoding];
            }
            id<MTLBlitCommandEncoder> blit=[command blitCommandEncoder];
            if(!blit)return finish(m,@"blit_encoder_failed",nil,26,json,cap);
            for(NSUInteger i=0;i<ntextures;i++)
                [blit copyFromTexture:textures[i] sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0)
                    sourceSize:MTLSizeMake(td.width,td.height,1) toBuffer:rb destinationOffset:GUARD+(route==0?i*STRIDE:0)
                    destinationBytesPerRow:STRIDE destinationBytesPerImage:STRIDE*td.height];
            [blit endEncoding];[command commit];[command waitUntilCompleted];
            e->status=(uint32_t)command.status;e->completed=command.status==MTLCommandBufferStatusCompleted;e->no_error=command.error==nil;
            m[@"command_status"]=@(command.status);m[@"command_completed"]=@(e->completed!=0);m[@"wait_until_completed_called"]=@YES;
            if(!e->completed || !e->no_error)return finish(m,@"command_failed",command.error,27,json,cap);
            if(!edge_guards(eb,count*16) || !edge_guards(hb,count*8))return finish(m,@"sidecar_guard_changed",nil,28,json,cap);
            const uint8_t *r=rb.contents;
            for(NSUInteger i=0;i<rb.length;i++) {
                BOOL edge=i<GUARD || i>=GUARD+rows*STRIDE;
                BOOL pad=!edge && (i-GUARD)%STRIDE>=payload_per_row;
                if((edge || pad) && r[i]!=0xa5)return finish(m,@"blit_guard_changed",nil,28,json,cap);
            }
            for(NSUInteger y=0;y<rows;y++)memcpy((uint8_t *)pixels+y*payload_per_row,r+GUARD+y*STRIDE,payload_per_row);
            memcpy(echo,(const uint8_t *)eb.contents+GUARD,count*16);
            memcpy(casts,(const uint8_t *)hb.contents+GUARD,count*8);
            e->copied=1;m[@"readback_copied"]=@YES;m[@"all_guards_intact"]=@YES;
            return finish(m,@"gpu_readback_observed",nil,0,json,cap);
        } @catch(NSException *exception) {
            m[@"exception_name"]=exception.name?:@"unknown";
            return finish(m,@"objective_c_exception",nil,29,json,cap);
        }
    }
}
