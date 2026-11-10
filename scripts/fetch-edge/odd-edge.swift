// Bounded owned Metal observation, reusing the fetch/stencil helper scaffold.
import Foundation
import Metal
import CryptoKit
import Darwin

struct Failure: Error { let reason:String }
func need(_ b:Bool,_ why:String)throws{if !b{throw Failure(reason:why)}}
func sha(_ d:Data)->String{SHA256.hash(data:d).map{String(format:"%02x",$0)}.joined()}
let W=35,H=27,N=945,guardBytes=256
let full:[SIMD2<Float>]=[SIMD2(-35,-27),SIMD2(105,-27),SIMD2(-35,81)]
struct Spec{
    let id:Int,name:String,v:[UInt32]
    var derivatives:Bool{v[0]==1};var x:Int{Int(v[1])};var y:Int{Int(v[2])}
    var pipelineKey:String{derivatives ? "derivative" : "scalar"}
    var fragment:String{derivatives ? "edge_derivative" : "edge_scalar"}
    var points:[SIMD2<Float>]{[SIMD2(Float(x),Float(y)),SIMD2(Float(x),Float(y+8)),SIMD2(Float(x+10),Float(y)),SIMD2(Float(x+10),Float(y+8))]}
}
func sizedPass()->MTLRenderPassDescriptor{
    let p=MTLRenderPassDescriptor();p.renderTargetWidth=W;p.renderTargetHeight=H;return p
}
var report:[String:Any]=["schema":1,"status":"FAILED_NOT_PASS","native_observation_only":true]
var commands:[[String:Any]]=[]
var observations:[[String:Any]]=[]
var caseAttempts:[[String:Any]]=[]
var caseFailures:[[String:Any]]=[]
func errorDetails(_ error:Error)->[String:Any]{
    if let f=error as? Failure{return ["reason":String(f.reason.prefix(128))]}
    let e=error as NSError
    return ["reason":"native_API_error","error_domain":String(e.domain.prefix(128)),
            "error_code":e.code,"error_description":String(decoding:e.localizedDescription.utf8.prefix(2048),as:UTF8.self)]
}
var hashes:[String:String]=[:]
var outputBytes=0
var drawCount=0
var root=URL(fileURLWithPath:CommandLine.arguments.last ?? ".")
func flush()throws{
    try need(caseAttempts.count<=8 && caseFailures.count<=8,"case_diagnostic_budget")
    report["case_attempts"]=caseAttempts;report["case_failures"]=caseFailures
    report["commands"]=commands;report["cases"]=observations;report["output_sha256"]=hashes
    report["output_bytes"]=outputBytes;report["draw_count"]=drawCount
    let data=try JSONSerialization.data(withJSONObject:report,options:[.sortedKeys,.prettyPrinted])
    try need(data.count<=1_048_576,"report_budget")
    try data.write(to:root.appendingPathComponent("native.json"),options:.atomic)
}
func save(_ name:String,_ data:Data)throws{
    try need(name.range(of:"^[a-z0-9.-]+\\.bin$",options:.regularExpression) != nil,"output_name")
    try need(hashes[name]==nil && hashes.count<16 && data.count<=1_048_576 && outputBytes+data.count<=1_048_576,"raw_output_budget")
    try data.write(to:root.appendingPathComponent(name),options:.withoutOverwriting)
    hashes[name]=sha(data);outputBytes+=data.count
}
func osBuild()throws->String{
    var count=0
    try need(sysctlbyname("kern.osversion",nil,&count,nil,0)==0 && count>1 && count<128,"OS_build_size")
    var value=[CChar](repeating:0,count:count)
    let ok=value.withUnsafeMutableBytes{sysctlbyname("kern.osversion",$0.baseAddress,&count,nil,0)}
    try need(ok==0,"OS_build_unavailable")
    return String(cString:value)
}
final class Readback{
    let buffer:MTLBuffer,bpp:Int,pitch:Int
    init(_ d:MTLDevice,_ bytesPerPixel:Int)throws{
        bpp=bytesPerPixel;pitch=((W*bpp+255)/256)*256
        guard let b=d.makeBuffer(length:512+pitch*H,options:.storageModeShared)else{throw Failure(reason:"readback_allocation")}
        buffer=b;memset(b.contents(),0xcd,b.length)
    }
    func raw()throws->Data{
        let bytes=[UInt8](Data(bytes:buffer.contents(),count:buffer.length))
        for i in bytes.indices{
            let local=i-guardBytes
            let payload=local>=0 && local<pitch*H && local%pitch<W*bpp
            try need(payload || bytes[i]==0xcd,"readback_guard_or_padding")
        }
        return Data(bytes)
    }
    func payload()throws->[UInt8]{
        let b=[UInt8](try raw());return (0..<H).flatMap{y in Array(b[(guardBytes+y*pitch)..<(guardBytes+y*pitch+W*bpp)])}
    }
}
func faceInfo(_ f:MTLStencilDescriptor?)->Any{
    guard let f=f else{return NSNull()}
    return ["compare":f.stencilCompareFunction.rawValue,"fail":f.stencilFailureOperation.rawValue,
            "depth_fail":f.depthFailureOperation.rawValue,"pass":f.depthStencilPassOperation.rawValue,
            "read_mask":f.readMask,"write_mask":f.writeMask] as [String:Any]
}
func depthInfo(_ d:MTLDepthStencilDescriptor)->[String:Any]{
    ["depth_compare":d.depthCompareFunction.rawValue,"depth_write":d.isDepthWriteEnabled,
     "front":faceInfo(d.frontFaceStencil),"back":faceInfo(d.backFaceStencil)]
}
func textureInfo(_ t:MTLTexture)->[String:Any]{
    ["format":t.pixelFormat.rawValue,"width":t.width,"height":t.height,"samples":t.sampleCount,
     "mips":t.mipmapLevelCount,"array_length":t.arrayLength,"storage":t.storageMode.rawValue,
     "usage":t.usage.rawValue,"type":t.textureType.rawValue,"framebuffer_only":t.isFramebufferOnly]
}
final class Harness{
    let device:MTLDevice,queue:MTLCommandQueue,library:MTLLibrary
    let queryPipeline:MTLRenderPipelineState,queryState:MTLDepthStencilState,seedState:MTLDepthStencilState
    var pipelines:[String:MTLRenderPipelineState]=[:]
    var pipelineRecords:[String:[String:Any]]=[:]
    var encoderLabels: Set<String> = []
    func command(_ q:MTLCommandQueue,_ name:String)throws->MTLCommandBuffer{
        let d=MTLCommandBufferDescriptor();d.errorOptions = .encoderExecutionStatus
        guard let cb=q.makeCommandBuffer(descriptor:d)else{throw Failure(reason:"command_buffer_unavailable")}
        cb.label=name;return cb
    }
    func mark(_ e:MTLCommandEncoder,_ cb:MTLCommandBuffer,_ role:String){
        let name=(cb.label ?? "unlabeled")+"/"+role
        e.label=name;encoderLabels.insert(name)
    }
    init(_ source:String)throws{
        guard let d=MTLCreateSystemDefaultDevice(),let q=d.makeCommandQueue()else{throw Failure(reason:"UNAVAILABLE_native_Metal")}
        device=d;queue=q
        report["os_version_and_build"]=ProcessInfo.processInfo.operatingSystemVersionString
        report["os_build"]=try osBuild()
        var info:[String:Any]=["name":d.name,"registry_id":String(d.registryID),"unified_memory":d.hasUnifiedMemory,
            "mac2":d.supportsFamily(.mac2),"apple7":d.supportsFamily(.apple7)]
        if #available(macOS 14.0,*){info["architecture"]=d.architecture.name}
        report["device"]=info
        try need(d.name.lowercased().contains("paravirtual"),"UNQUALIFIED_device_not_Apple_Paravirtual")
        let options=MTLCompileOptions();let defaultFast=options.fastMathEnabled;options.fastMathEnabled=true
        var compiler:[String:Any]=["default_fast_math":defaultFast,"effective_fast_math":options.fastMathEnabled,
            "language_version":options.languageVersion.rawValue,"preserve_invariance":options.preserveInvariance]
        if #available(macOS 15.0,*){compiler["math_mode"]=options.mathMode.rawValue;compiler["math_functions"]=options.mathFloatingPointFunctions.rawValue}
        report["compiler"]=compiler;report["source_sha256"]=sha(Data(source.utf8))
        let l=try d.makeLibrary(source:source,options:options);library=l
        let p=MTLRenderPipelineDescriptor();p.vertexFunction=l.makeFunction(name:"edge_vertex");p.fragmentFunction=l.makeFunction(name:"edge_query")
        p.colorAttachments[0].pixelFormat = .rgba8Uint;p.colorAttachments[0].writeMask = .all
        p.rasterSampleCount=1;p.stencilAttachmentPixelFormat = .stencil8;p.depthAttachmentPixelFormat = .invalid
        queryPipeline=try d.makeRenderPipelineState(descriptor:p)
        func ds(_ comparison:MTLCompareFunction,_ operation:MTLStencilOperation,_ read:UInt32,_ write:UInt32)throws->(MTLDepthStencilState,MTLDepthStencilDescriptor){
            let f=MTLStencilDescriptor();f.stencilCompareFunction=comparison;f.stencilFailureOperation = .keep;f.depthFailureOperation = .keep
            f.depthStencilPassOperation=operation;f.readMask=read;f.writeMask=write
            let v=MTLDepthStencilDescriptor();v.depthCompareFunction = .always;v.isDepthWriteEnabled=false;v.frontFaceStencil=f;v.backFaceStencil=f
            guard let s=d.makeDepthStencilState(descriptor:v)else{throw Failure(reason:"stencil_state_allocation")};return(s,v)
        }
        let query=try ds(.equal,.keep,255,0);queryState=query.0;report["query_state"]=depthInfo(query.1)
        let seed=try ds(.always,.replace,255,255);seedState=seed.0;report["seed_state"]=depthInfo(seed.1)
    }
    func tex(_ f:MTLPixelFormat)throws->MTLTexture{
        let desc=MTLTextureDescriptor.texture2DDescriptor(pixelFormat:f,width:W,height:H,mipmapped:false)
        desc.storageMode = .private;desc.usage = .renderTarget
        guard let t=device.makeTexture(descriptor:desc)else{throw Failure(reason:"texture_allocation")}
        try need(t.width==W && t.height==H && t.pixelFormat==f && t.sampleCount==1 && t.mipmapLevelCount==1 && t.arrayLength==1 && t.storageMode == .private && t.usage == .renderTarget  &&  !t.isFramebufferOnly,"actual_texture_contract")
        return t
    }
    func finish(_ cb:MTLCommandBuffer,_ name:String)throws{
        cb.commit();cb.waitUntilCompleted()
        var observation:[String:Any]=["name":name,"committed":true,"wait_returned":true,"status":cb.status.rawValue,
            "completed":cb.status == .completed,"error_present":cb.error != nil,"error_options":cb.errorOptions.rawValue]
        if let error=cb.error {
            let e=error as NSError
            observation["error_domain"]=String(e.domain.prefix(128));observation["error_code"]=e.code
            observation["error_description"]=String(decoding:e.localizedDescription.utf8.prefix(2048),as:UTF8.self)
            if let infos=e.userInfo[MTLCommandBufferEncoderInfoErrorKey] as? [MTLCommandBufferEncoderInfo] {
                observation["encoder_info_count"]=infos.count;observation["encoder_info_truncated"]=infos.count>16
                observation["encoder_states"]=infos.prefix(16).map{info -> [String:Any] in
                    ["label":encoderLabels.contains(info.label) ? info.label : "unrecognized-redacted",
                     "error_state_raw":info.errorState.rawValue,"error_state":String(String(describing:info.errorState).prefix(64))]
                }
            }else{observation["encoder_states_unavailable"]=true}
            // No other userInfo values, signposts, environment data or memory dumps.
        }
        commands.append(observation)
        try flush();try need(cb.status == .completed && cb.error==nil,"native_command_failed")
    }
    func copy(_ b:MTLBlitCommandEncoder,_ t:MTLTexture,_ out:Readback){
        b.copy(from:t,sourceSlice:0,sourceLevel:0,sourceOrigin:MTLOrigin(x:0,y:0,z:0),sourceSize:MTLSize(width:W,height:H,depth:1),
            to:out.buffer,destinationOffset:guardBytes,destinationBytesPerRow:out.pitch,destinationBytesPerImage:out.pitch*H)
    }
    func bind(_ e:MTLRenderCommandEncoder,_ points:[SIMD2<Float>],_ scissor:MTLScissorRect){
        e.setViewport(MTLViewport(originX:0,originY:0,width:Double(W),height:Double(H),znear:0,zfar:1));e.setCullMode(.none)
        e.setFrontFacing(.counterClockwise);e.setTriangleFillMode(.fill);e.setScissorRect(scissor)
        points.withUnsafeBytes{e.setVertexBytes($0.baseAddress!,length:$0.count,index:0)}
    }
    func draw(_ e:MTLRenderCommandEncoder,_ type:MTLPrimitiveType,_ count:Int)throws{
        drawCount+=1;try need(drawCount<=8000,"native_draw_budget")
        e.drawPrimitives(type:type,vertexStart:0,vertexCount:count)
    }
    func seedStencil(_ cb:MTLCommandBuffer,_ st:MTLTexture,_ q:MTLTexture,_ ramp:Bool=false)throws{
        let p=sizedPass();p.colorAttachments[0].texture=q;p.colorAttachments[0].loadAction = .clear;p.colorAttachments[0].storeAction = .store
        p.stencilAttachment.texture=st;p.stencilAttachment.loadAction = .clear;p.stencilAttachment.storeAction = .store;p.stencilAttachment.clearStencil=43
        guard let e=cb.makeRenderCommandEncoder(descriptor:p)else{throw Failure(reason:"stencil_seed_encoder")}
        mark(e,cb,"seed-render");e.setRenderPipelineState(queryPipeline);e.setDepthStencilState(seedState)
        if ramp{
            for y in 0..<H{for x in 0..<W{
                let value=UInt32((y*W+x)&255);var v=SIMD4<UInt32>(value,0,0,0)
                e.setStencilReferenceValue(value);e.setFragmentBytes(&v,length:16,index:0)
                bind(e,full,MTLScissorRect(x:x,y:y,width:1,height:1));try draw(e,.triangle,3)
            }}
        }
        e.endEncoding()
    }
    func query(_ cb:MTLCommandBuffer,_ st:MTLTexture,_ q:MTLTexture)throws{
        let p=sizedPass();p.colorAttachments[0].texture=q;p.colorAttachments[0].loadAction = .clear;p.colorAttachments[0].storeAction = .store
        p.colorAttachments[0].clearColor=MTLClearColorMake(0,0,0,0);p.stencilAttachment.texture=st;p.stencilAttachment.loadAction = .load;p.stencilAttachment.storeAction = .store
        guard let e=cb.makeRenderCommandEncoder(descriptor:p)else{throw Failure(reason:"stencil_query_encoder")}
        mark(e,cb,"stencil-query-sweep")
        e.setRenderPipelineState(queryPipeline);e.setDepthStencilState(queryState);bind(e,full,MTLScissorRect(x:0,y:0,width:W,height:H))
        for i in 0..<256{let ref=UInt32(i);var p=SIMD4<UInt32>(ref,0,0,0);e.setStencilReferenceValue(ref);e.setFragmentBytes(&p,length:16,index:0);try draw(e,.triangle,3)}
        e.endEncoding()
    }
    func calibrate()throws{
        let st=try tex(.stencil8),q=try tex(.rgba8Uint),r=try Readback(device,4)
        let cb=try command(queue,"all256-query-calibration")
        try seedStencil(cb,st,q,true);try query(cb,st,q)
        guard let blit=cb.makeBlitCommandEncoder()else{throw Failure(reason:"calibration_readback")};mark(blit,cb,"calibration-readback");copy(blit,q,r);blit.endEncoding();try finish(cb,"all256-query-calibration")
        let p=try r.payload()
        for i in 0..<N{let v=UInt8(i&255);try need(Array(p[i*4..<i*4+4])==[v,255-v,83,255],"all256_query_calibration_failed")}
        try save("query-calibration.bin",try r.raw());report["query_calibration_passed"]=true;try flush()
    }
    func state()throws->(MTLDepthStencilState,MTLDepthStencilDescriptor){
        let f=MTLStencilDescriptor();f.stencilCompareFunction = .always
        f.stencilFailureOperation = .keep;f.depthFailureOperation = .keep;f.depthStencilPassOperation = .keep
        f.readMask=UInt32.max;f.writeMask=UInt32.max
        let d=MTLDepthStencilDescriptor();d.depthCompareFunction = .always;d.isDepthWriteEnabled=false
        d.frontFaceStencil=f;d.backFaceStencil=f
        guard let s=device.makeDepthStencilState(descriptor:d)else{throw Failure(reason:"consumer_stencil_state")};return(s,d)
    }
    func pipeline(_ c:Spec)throws->MTLRenderPipelineState{
        let key=c.pipelineKey;if let p=pipelines[key]{return p}
        let d=MTLRenderPipelineDescriptor();d.vertexFunction=library.makeFunction(name:"edge_vertex");d.fragmentFunction=library.makeFunction(name:c.fragment)
        d.rasterSampleCount=1;d.isAlphaToCoverageEnabled=false;d.isAlphaToOneEnabled=false
        d.stencilAttachmentPixelFormat = .stencil8;d.depthAttachmentPixelFormat = .invalid
        for (i,f) in [MTLPixelFormat.rgba16Float,MTLPixelFormat.rg16Float].enumerated(){
            d.colorAttachments[i].pixelFormat=f;d.colorAttachments[i].isBlendingEnabled=false
            d.colorAttachments[i].writeMask = i==0 ? .all : []
        }
        try need(d.colorAttachments[1].writeMask.rawValue==0 && d.colorAttachments[2].pixelFormat == .invalid,"direct_topology_descriptor")
        let p=try device.makeRenderPipelineState(descriptor:d);pipelines[key]=p
        pipelineRecords[key]=["formats0to2":(0..<3).map{d.colorAttachments[$0].pixelFormat.rawValue},
            "write_masks0to2":(0..<3).map{d.colorAttachments[$0].writeMask.rawValue},"fragment":d.fragmentFunction!.name,
            "stencil_format":d.stencilAttachmentPixelFormat.rawValue,"depth_format":d.depthAttachmentPixelFormat.rawValue,
            "samples":d.rasterSampleCount,"alpha_to_coverage":d.isAlphaToCoverageEnabled,"alpha_to_one":d.isAlphaToOneEnabled]
        return p
    }
    func run(_ c:Spec,_ seedColor:Data,_ seedFetch:Data)throws{
        guard let caseQueue=device.makeCommandQueue()else{throw Failure(reason:"fresh_case_queue_unavailable")}
        caseQueue.label="case\(c.id)-fresh-queue"
        let color=try tex(.rgba16Float),fetch=try tex(.rg16Float),st=try tex(.stencil8),q=try tex(.rgba8Uint)
        let outputs=try [8,4,4,8,4,4].map{try Readback(device,$0)}
        let uploadColor=try Readback(device,8),uploadFetch=try Readback(device,4)
        for (upload,data) in [(uploadColor,seedColor),(uploadFetch,seedFetch)]{
            data.withUnsafeBytes{s in for y in 0..<H{memcpy(upload.buffer.contents().advanced(by:guardBytes+y*upload.pitch),s.baseAddress!.advanced(by:y*W*upload.bpp),W*upload.bpp)}}
        }
        let first=try command(caseQueue,"case\(c.id)-seed-and-query")
        guard let up=first.makeBlitCommandEncoder()else{throw Failure(reason:"initial_command")}
        mark(up,first,"seed-upload")
        for (upload,texture) in [(uploadColor,color),(uploadFetch,fetch)]{
            up.copy(from:upload.buffer,sourceOffset:guardBytes,sourceBytesPerRow:upload.pitch,sourceBytesPerImage:upload.pitch*H,
                sourceSize:MTLSize(width:W,height:H,depth:1),to:texture,destinationSlice:0,destinationLevel:0,destinationOrigin:MTLOrigin(x:0,y:0,z:0))
        }
        up.endEncoding();try seedStencil(first,st,q);try query(first,st,q)
        guard let initial=first.makeBlitCommandEncoder()else{throw Failure(reason:"initial_capture")}
        mark(initial,first,"seed-readback");copy(initial,color,outputs[0]);copy(initial,fetch,outputs[1]);copy(initial,q,outputs[2]);initial.endEncoding()
        try finish(first,"case\(c.id)-seed-and-query")
        let uploadedColor=try uploadColor.payload(),uploadedFetch=try uploadFetch.payload()
        try need(Data(uploadedColor)==seedColor && Data(uploadedFetch)==seedFetch,"upload_input_changed")
        let second=try command(caseQueue,"case\(c.id)-consumer-and-query")
        let pass=sizedPass()
        for(i,t) in [color,fetch].enumerated(){pass.colorAttachments[i].texture=t;pass.colorAttachments[i].loadAction = .load;pass.colorAttachments[i].storeAction = .store}
        pass.stencilAttachment.texture=st;pass.stencilAttachment.loadAction = .load;pass.stencilAttachment.storeAction = .store
        let ds=try state();let ps=try pipeline(c)
        guard let e=second.makeRenderCommandEncoder(descriptor:pass)else{throw Failure(reason:"consumer_encoder")}
        mark(e,second,"consumer-strip");e.setRenderPipelineState(ps);e.setDepthStencilState(ds.0);e.setStencilReferenceValue(117)
        bind(e,c.points,MTLScissorRect(x:c.x,y:c.y,width:10,height:8));try draw(e,.triangleStrip,4)
        e.endEncoding();try query(second,st,q)
        guard let read=second.makeBlitCommandEncoder()else{throw Failure(reason:"final_readback")}
        mark(read,second,"final-readback");copy(read,color,outputs[3]);copy(read,fetch,outputs[4]);copy(read,q,outputs[5]);read.endEncoding()
        try finish(second,"case\(c.id)-consumer-and-query")
        var raw=Data();for r in outputs{raw.append(try r.raw())};try need(raw.count==58368,"case_readback_layout")
        let name=String(format:"case%02d.bin",c.id);try save(name,raw)
        observations.append(["id":c.id,"name":c.name,"file":name,"bytes":raw.count,"sha256":sha(raw),
            "consumer_stencil_attached":true,"explicit_both_AlwaysKeep_faces":true,"explicit_depth_stencil_state_bound":true,
            "state_descriptor":depthInfo(ds.1),"reference_front_back":117,"fragment":c.fragment,"early_attribute_requested":true,
            "derivative_variant":c.derivatives,"derivative_operand":"interpolated varying only","source_framebuffer_fetch_slots":[1],
            "logical_strip_draws":1,"physical_consumer_draw_calls":1,"split_triangle_calls":false,"same_consumer_encoder":true,
            "color0_write_mask":15,"all_MRT_blending":false,"scissor":[c.x,c.y,10,8],"points":c.points.map{[$0.x,$0.y]},
            "viewport":[0,0,W,H],"render_target_size_getters":[pass.renderTargetWidth,pass.renderTargetHeight],
            "samples":1,"depth_attachment":false,"depth_write":false,"discard":false,"control_flow":false,
            "actual_textures":[color,fetch,st,q].map{textureInfo($0)},"color_attachment_formats":[color.pixelFormat.rawValue,fetch.pixelFormat.rawValue],
            "color_attachment_write_masks":[15,0],"color_load_actions":(0..<2).map{pass.colorAttachments[$0].loadAction.rawValue},
            "color_store_actions":(0..<2).map{pass.colorAttachments[$0].storeAction.rawValue},"stencil_load_action":pass.stencilAttachment.loadAction.rawValue,
            "stencil_store_action":pass.stencilAttachment.storeAction.rawValue,"no_color2_attachment":pass.colorAttachments[2].texture == nil,
            "pipeline_descriptor_getters":pipelineRecords[c.pipelineKey]!,"readback_pitches":outputs.map{$0.pitch},
            "fresh_case_resources_and_queue":true,"guards_checked":true,"before_and_after_stencil":"GPU Equal/Keep sweep; no direct stencil-blit assumption"])
        try flush()
    }
}
func run()throws{
    try need(CommandLine.arguments.count==3 && MemoryLayout<SIMD2<Float>>.stride==8,"usage_or_host_MSL_ABI")
    let source=try String(contentsOfFile:CommandLine.arguments[1],encoding:.utf8)
    let input=try Data(contentsOf:root.appendingPathComponent("cases.tsv"))
    let seedColor=try Data(contentsOf:root.appendingPathComponent("seed-rgba16.bin"))
    let seedFetch=try Data(contentsOf:root.appendingPathComponent("seed-rg16.bin"))
    try need(source.utf8.count<=65536 && input.count<8192 && seedColor.count==7560 && seedFetch.count==3780,"owned_input_shape")
    let lines=String(decoding:input,as:UTF8.self).split(separator:"\n")
    try need(lines.count==9 && lines[0]=="id\tname\tderivatives\tx\ty\twidth\theight","case_header_count")
    let names=["interior","right","bottom","corner"],xs=[3,25,3,25],ys=[5,5,19,19]
    var specs:[Spec]=[]
    for(id,line) in lines.dropFirst().enumerated(){
        let f=line.split(separator:"\t");try need(f.count==7 && Int(f[0])==id,"case_row")
        let v=f.dropFirst(2).compactMap{UInt32($0)};try need(v.count==5,"case_numeric_fields")
        try need(v[0]==UInt32(id%2) && v[1]==UInt32(xs[id/2]) && v[2]==UInt32(ys[id/2]) && v[3]==10 && v[4]==8,"case_domain")
        let name=names[id/2]+(v[0]==1 ? "-derivative" : "-scalar");try need(String(f[1])==name,"case_name")
        specs.append(Spec(id:id,name:name,v:v))
    }
    report["case_manifest_sha256"]=sha(input);report["color_seed_sha256"]=sha(seedColor);report["fetch_seed_sha256"]=sha(seedFetch)
    report["contract"]=["width":W,"height":H,"case_count":8,"positive_cases":8,"negative_controls":"wrong expected model only; no extra native cases",
        "framebuffer_input":"half2 color(1)","interpolated_varying":"x/16+y/32; ordinary interpolation with constant reciprocal W=1",
        "derivative_operand":"varying only, never framebuffer fetch","derivative_precision":"float32; half4 color0 return",
        "source_declared_fetch_slots":[1],"all_cases_explicit_AlwaysKeep":true,
        "all_cases_early_attribute":true,"no_device_buffer_fragment_side_effects":true,"shader_depth_output":false,"shader_stencil_output":false,
        "discard_supported":false,"control_flow_supported":false,"per_case_layout":"before RGBA16; before RG16; before query; after RGBA16; after RG16; after query",
        "guard_prefix_suffix":256,"poison_byte":205,"row_alignment":256,"case_bytes":58368,"modeled_OOB_fetch_values":false,
        "private_texture_physical_pitch_asserted":false,"invocation_counts_measured":false,"model_qualification_not_inferred_from_completion":true] as [String:Any]
    let h=try Harness(source)
    report["diagnostic_mode"]="encoderExecutionStatus; fresh per-case mutable resources/queue; no retry or expected-model change"
    do{try h.calibrate()}catch{report["calibration_failure"]=errorDetails(error);try flush()}
    for c in specs{
        let firstCommand=commands.count
        let prior=commands.contains{($0["completed"] as? Bool) != true || ($0["error_present"] as? Bool)==true}
        caseAttempts.append(["id":c.id,"name":c.name,"derivative_variant":c.derivatives,"fresh_case_resources_and_queue_requested":true,
            "prior_command_error":prior,"context_health":prior ? "unknown_after_prior_command_error" : "no_prior_command_error_observed",
            "commands_before":firstCommand,"outcome":"attempting"])
        try flush()
        do{try h.run(c,seedColor,seedFetch);caseAttempts[caseAttempts.count-1]["outcome"]="captured"}
        catch{
            caseFailures.append(["id":c.id,"name":c.name,"error":errorDetails(error),"commands_before":firstCommand,"commands_after":commands.count,
                "prior_command_error":prior,"semantic_failure_inference":prior ? "unknown_possible_poisoned_device_context" : "not_inferred_from_API_failure"])
            caseAttempts[caseAttempts.count-1]["outcome"]="failed"
        }
        caseAttempts[caseAttempts.count-1]["commands_after"]=commands.count;try flush()
    }
    let allBuffers=commands.count==17 && commands.allSatisfy{($0["completed"] as? Bool)==true && ($0["error_present"] as? Bool)==false}
    let allCases=caseAttempts.count==8 && observations.count==8 && caseFailures.isEmpty
    report["all8_cases_succeeded"]=allCases;report["all17_command_buffers_succeeded"]=allBuffers;try flush()
    try need((report["query_calibration_passed"] as? Bool)==true && allCases && allBuffers && drawCount==5305 && outputBytes==474368,"fail_closed_all8_all17_required")
    report["status"]="NATIVE_OBSERVATIONS_COMPLETE";try flush()
}
var exitCode:Int32=0
do{try run()}catch{
    exitCode=3;report["top_level_error"]=errorDetails(error)
    if let f=error as? Failure{report["failure"]=f.reason}else{report["failure"]="native_API_error"}
    report["status"]="FAILED_NOT_PASS";try? flush()
}
print("native_observation_exit=\(exitCode) model_qualification_is_separate=true")
exit(exitCode)
