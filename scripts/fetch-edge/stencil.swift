// Bounded public Metal observation, adapted from the owned fetch/stencil helpers.
import Foundation
import Metal
import CryptoKit
import Darwin

struct Failure: Error { let reason:String }
func need(_ b:Bool,_ why:String)throws{if !b{throw Failure(reason:why)}}
func sha(_ d:Data)->String{SHA256.hash(data:d).map{String(format:"%02x",$0)}.joined()}
let W=32,H=32,N=1024,guardBytes=256
let rect:[SIMD2<Float>]=[SIMD2(3,5),SIMD2(3,20),SIMD2(21,5),SIMD2(21,20)]
let full:[SIMD2<Float>]=[SIMD2(-32,-32),SIMD2(96,-32),SIMD2(-32,96)]
struct Params{var tag:UInt32;var unused0:UInt32=0;var unused1:UInt32=0;var unused2:UInt32=0}
struct Spec{
    let id:Int,name:String,v:[UInt32]
    var attached:Bool{v[0]==1};var comparison:UInt32{v[1]};var operation:UInt32{v[2]}
    var early:Bool{v[3]==1};var discard:UInt32{v[4]};var draws:Int{Int(v[5])}
    var colorMask:UInt32{v[6]};var split:Bool{v[7]==1};var checker:Bool{v[8]==1}
    var reference:UInt32{v[9]};var readMask:UInt32{v[10]};var writeMask:UInt32{v[11]}
    var negative:UInt32{v[12]}
    var visible:Bool{id>=10}
    var pipelineKey:String{"\(early)-\(attached)-\(colorMask)-\(visible)"}
}
var report:[String:Any]=["schema":1,"status":"FAILED_NOT_PASS","native_observation_only":true]
var commands:[[String:Any]]=[]
var observations:[[String:Any]]=[]
var hashes:[String:String]=[:]
var outputBytes=0
var drawCount=0
var root=URL(fileURLWithPath:CommandLine.arguments.last ?? ".")
func flush()throws{
    report["commands"]=commands;report["cases"]=observations;report["output_sha256"]=hashes
    report["output_bytes"]=outputBytes;report["draw_count"]=drawCount
    let data=try JSONSerialization.data(withJSONObject:report,options:[.sortedKeys,.prettyPrinted])
    try need(data.count<=1_048_576,"report_budget")
    try data.write(to:root.appendingPathComponent("native.json"),options:.atomic)
}
func save(_ name:String,_ data:Data)throws{
    try need(name.range(of:"^[a-z0-9.-]+\\.bin$",options:.regularExpression) != nil,"output_name")
    try need(hashes[name]==nil && hashes.count<32 && data.count<=1_048_576 && outputBytes+data.count<=2_097_152,"raw_output_budget")
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
        let p=MTLRenderPipelineDescriptor();p.vertexFunction=l.makeFunction(name:"stencil_vertex");p.fragmentFunction=l.makeFunction(name:"stencil_query")
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
        commands.append(["name":name,"committed":true,"wait_returned":true,"status":cb.status.rawValue,
            "completed":cb.status == .completed,"error_present":cb.error != nil])
        try flush();try need(cb.status == .completed && cb.error==nil,"native_command_failed")
    }
    func copy(_ b:MTLBlitCommandEncoder,_ t:MTLTexture,_ out:Readback){
        b.copy(from:t,sourceSlice:0,sourceLevel:0,sourceOrigin:MTLOrigin(x:0,y:0,z:0),sourceSize:MTLSize(width:W,height:H,depth:1),
            to:out.buffer,destinationOffset:guardBytes,destinationBytesPerRow:out.pitch,destinationBytesPerImage:out.pitch*H)
    }
    func bind(_ e:MTLRenderCommandEncoder,_ points:[SIMD2<Float>],_ scissor:MTLScissorRect){
        e.setViewport(MTLViewport(originX:0,originY:0,width:32,height:32,znear:0,zfar:1));e.setCullMode(.none)
        e.setFrontFacing(.counterClockwise);e.setTriangleFillMode(.fill);e.setScissorRect(scissor)
        points.withUnsafeBytes{e.setVertexBytes($0.baseAddress!,length:$0.count,index:0)}
    }
    func draw(_ e:MTLRenderCommandEncoder,_ type:MTLPrimitiveType,_ count:Int)throws{
        drawCount+=1;try need(drawCount<=16000,"native_draw_budget")
        e.drawPrimitives(type:type,vertexStart:0,vertexCount:count)
    }
    func seedStencil(_ cb:MTLCommandBuffer,_ st:MTLTexture,_ q:MTLTexture,_ checker:Bool,_ ramp:Bool=false)throws{
        let p=MTLRenderPassDescriptor();p.colorAttachments[0].texture=q;p.colorAttachments[0].loadAction = .clear;p.colorAttachments[0].storeAction = .store
        p.stencilAttachment.texture=st;p.stencilAttachment.loadAction = .clear;p.stencilAttachment.storeAction = .store;p.stencilAttachment.clearStencil=checker ? 42 : 43
        guard let e=cb.makeRenderCommandEncoder(descriptor:p)else{throw Failure(reason:"stencil_seed_encoder")}
        e.setRenderPipelineState(queryPipeline);e.setDepthStencilState(seedState)
        if checker || ramp{
            for y in 0..<H{for x in 0..<W where ramp  ||  ((x+y)&1)==1{
                let value:UInt32=ramp ? UInt32((y*W+x)&255) : 43
                var v=SIMD4<UInt32>(value,0,0,0);e.setStencilReferenceValue(value);e.setFragmentBytes(&v,length:16,index:0)
                bind(e,full,MTLScissorRect(x:x,y:y,width:1,height:1));try draw(e,.triangle,3)
            }}
        }
        e.endEncoding()
    }
    func query(_ cb:MTLCommandBuffer,_ st:MTLTexture,_ q:MTLTexture)throws{
        let p=MTLRenderPassDescriptor();p.colorAttachments[0].texture=q;p.colorAttachments[0].loadAction = .clear;p.colorAttachments[0].storeAction = .store
        p.colorAttachments[0].clearColor=MTLClearColorMake(0,0,0,0);p.stencilAttachment.texture=st;p.stencilAttachment.loadAction = .load;p.stencilAttachment.storeAction = .store
        guard let e=cb.makeRenderCommandEncoder(descriptor:p)else{throw Failure(reason:"stencil_query_encoder")}
        e.setRenderPipelineState(queryPipeline);e.setDepthStencilState(queryState);bind(e,full,MTLScissorRect(x:0,y:0,width:W,height:H))
        for i in 0..<256{let ref=UInt32(i);var p=SIMD4<UInt32>(ref,0,0,0);e.setStencilReferenceValue(ref);e.setFragmentBytes(&p,length:16,index:0);try draw(e,.triangle,3)}
        e.endEncoding()
    }
    func calibrate()throws{
        let st=try tex(.stencil8),q=try tex(.rgba8Uint),r=try Readback(device,4)
        guard let cb=queue.makeCommandBuffer()else{throw Failure(reason:"calibration_command")}
        try seedStencil(cb,st,q,false,true);try query(cb,st,q)
        guard let blit=cb.makeBlitCommandEncoder()else{throw Failure(reason:"calibration_readback")};copy(blit,q,r);blit.endEncoding();try finish(cb,"all256-query-calibration")
        let p=try r.payload()
        for i in 0..<N{let v=UInt8(i&255);try need(Array(p[i*4..<i*4+4])==[v,255-v,83,255],"all256_query_calibration_failed")}
        try save("query-calibration.bin",try r.raw());report["query_calibration_passed"]=true;try flush()
    }
    func state(_ c:Spec)throws->(MTLDepthStencilState,MTLDepthStencilDescriptor){
        let f=MTLStencilDescriptor();f.stencilCompareFunction=c.comparison==0 ? .always : .notEqual
        f.stencilFailureOperation = .keep;f.depthFailureOperation = .keep;f.depthStencilPassOperation=c.operation==0 ? .keep : c.operation==1 ? .zero : .replace
        f.readMask=c.readMask;f.writeMask=c.writeMask
        let d=MTLDepthStencilDescriptor();d.depthCompareFunction = .always;d.isDepthWriteEnabled=false;d.frontFaceStencil=f;d.backFaceStencil=f
        guard let s=device.makeDepthStencilState(descriptor:d)else{throw Failure(reason:"consumer_stencil_state")};return(s,d)
    }
    func pipeline(_ c:Spec)throws->MTLRenderPipelineState{
        let key=c.pipelineKey;if let p=pipelines[key]{return p}
        let d=MTLRenderPipelineDescriptor();d.vertexFunction=library.makeFunction(name:"stencil_vertex");d.fragmentFunction=library.makeFunction(name:c.visible ? "stencil_visible_early" : c.early ? "stencil_early" : "stencil_default")
        d.rasterSampleCount=1;d.isAlphaToCoverageEnabled=false;d.isAlphaToOneEnabled=false
        d.stencilAttachmentPixelFormat=c.attached ? .stencil8 : .invalid;d.depthAttachmentPixelFormat = .invalid
        let formats:[MTLPixelFormat]=c.visible ? [.bgra8Unorm,.rg16Float] : [.bgra8Unorm,.rgba32Float,.rgba32Float]
        for (i,f) in formats.enumerated(){d.colorAttachments[i].pixelFormat=f;d.colorAttachments[i].isBlendingEnabled=false;d.colorAttachments[i].writeMask = c.visible && i==1 ? [] : .all}
        if c.visible{try need(d.colorAttachments[1].writeMask.rawValue==0 && d.colorAttachments[2].pixelFormat == .invalid,"visible_topology_descriptor")}
        let p=try device.makeRenderPipelineState(descriptor:d);pipelines[key]=p
        pipelineRecords[key]=["formats0to2":(0..<3).map{d.colorAttachments[$0].pixelFormat.rawValue},
            "write_masks0to2":(0..<3).map{d.colorAttachments[$0].writeMask.rawValue},"fragment":d.fragmentFunction!.name,
            "stencil_format":d.stencilAttachmentPixelFormat.rawValue,"depth_format":d.depthAttachmentPixelFormat.rawValue,
            "samples":d.rasterSampleCount]
        return p
    }
    func run(_ c:Spec,_ seed:Data)throws{
        let color=try tex(.bgra8Unorm),a=try tex(c.visible ? .rg16Float : .rgba32Float),st=try tex(.stencil8),q=try tex(.rgba8Uint)
        var colors=[color,a];if !c.visible{colors.append(try tex(.rgba32Float))}
        let bpps:[Int]=c.visible ? [4,4,4,4,4] : [4,4,4,16,16,4]
        let outputs=try bpps.map{try Readback(device,$0)}
        let upload=try Readback(device,4);seed.withUnsafeBytes{s in for y in 0..<H{memcpy(upload.buffer.contents().advanced(by:guardBytes+y*upload.pitch),s.baseAddress!.advanced(by:y*W*4),W*4)}}
        guard let first=queue.makeCommandBuffer(),let up=first.makeBlitCommandEncoder()else{throw Failure(reason:"initial_command")}
        up.copy(from:upload.buffer,sourceOffset:guardBytes,sourceBytesPerRow:upload.pitch,sourceBytesPerImage:upload.pitch*H,sourceSize:MTLSize(width:W,height:H,depth:1),to:color,destinationSlice:0,destinationLevel:0,destinationOrigin:MTLOrigin(x:0,y:0,z:0));up.endEncoding()
        try seedStencil(first,st,q,c.checker);try query(first,st,q)
        guard let initial=first.makeBlitCommandEncoder()else{throw Failure(reason:"initial_capture")};copy(initial,color,outputs[0]);copy(initial,q,outputs[1]);initial.endEncoding()
        try finish(first,"case\(c.id)-seed-and-query");try need(Data(try upload.payload())==seed,"upload_input_changed")
        guard let second=queue.makeCommandBuffer()else{throw Failure(reason:"consumer_command")}
        let pass=MTLRenderPassDescriptor()
        for(i,t) in colors.enumerated(){pass.colorAttachments[i].texture=t;pass.colorAttachments[i].loadAction=i==0 ? .load : .clear;pass.colorAttachments[i].storeAction = .store;pass.colorAttachments[i].clearColor=MTLClearColorMake(-999,-999,-999,-999)}
        if c.visible{pass.colorAttachments[1].clearColor=MTLClearColorMake(0.25,0.75,0,1)}
        if c.attached{pass.stencilAttachment.texture=st;pass.stencilAttachment.loadAction = .load;pass.stencilAttachment.storeAction = .store}
        let ds=try state(c);let ps=try pipeline(c)
        guard let e=second.makeRenderCommandEncoder(descriptor:pass)else{throw Failure(reason:"consumer_encoder")}
        e.setRenderPipelineState(ps);e.setDepthStencilState(c.attached ? ds.0 : nil);e.setStencilReferenceValue(c.reference)
        let sc=MTLScissorRect(x:3,y:5,width:18,height:15)
        for drawIndex in 0..<c.draws{
            var params=Params(tag:UInt32(drawIndex+1))
            e.setFragmentBytes(&params,length:16,index:0)
            bind(e,rect,sc);try draw(e,.triangleStrip,4)
        }
        e.endEncoding();try query(second,st,q)
        guard let read=second.makeBlitCommandEncoder()else{throw Failure(reason:"final_readback")}
        for(index,t)in colors.enumerated(){copy(read,t,outputs[index+2])};copy(read,q,outputs.last!)
        read.endEncoding();try finish(second,"case\(c.id)-consumer-and-query")
        var raw=Data();for r in outputs{raw.append(try r.raw())};try need(raw.count==(c.visible ? 43520 : 68608),"case_readback_layout")
        let name=String(format:"case%02d.bin",c.id);try save(name,raw)
        observations.append(["id":c.id,"name":c.name,"file":name,"bytes":raw.count,"sha256":sha(raw),"negative_control":c.negative,
            "consumer_stencil_attached":c.attached,"state_descriptor":depthInfo(ds.1),"reference_front_back":c.reference,
            "fragment":c.visible ? "stencil_visible_early" : c.early ? "stencil_early" : "stencil_default","early_attribute_requested":c.early,"discard_even_x":c.discard,
            "logical_strip_draws":c.draws,"split_triangle_calls":c.split,"same_consumer_encoder":true,
            "physical_consumer_draw_calls":c.split ? 2 : c.draws,"color0_write_mask":c.colorMask,"all_MRT_blending":false,
            "scissor":[3,5,18,15],"points":rect.map{[$0.x,$0.y]},"samples":1,"depth_attachment":false,"depth_write":false,
            "actual_textures":(colors+[st,q]).map{textureInfo($0)},"visible_color_derivative_control":c.visible,
            "color_attachment_formats":colors.map{$0.pixelFormat.rawValue},"color_attachment_write_masks":c.visible ? [15,0] : [15,15,15],
            "no_color2_attachment":pass.colorAttachments[2].texture == nil,"pipeline_descriptor_getters":pipelineRecords[c.pipelineKey]!,"RG16_clear_half_words":c.visible ? [13312,14848] : [],
            "guards_checked":true,"before_and_after_stencil":"GPU Equal/Keep sweep; no direct stencil-blit assumption"])
        try flush()
    }
}
func run()throws{
    try need(CommandLine.arguments.count==3,"usage_owned_metal_results")
    try need(MemoryLayout<Params>.size==16 && MemoryLayout<SIMD2<Float>>.stride==8,"host_MSL_ABI")
    let source=try String(contentsOfFile:CommandLine.arguments[1],encoding:.utf8)
    let input=try Data(contentsOf:root.appendingPathComponent("cases.tsv"));let seed=try Data(contentsOf:root.appendingPathComponent("seed-bgra.bin"))
    try need(input.count<8192 && seed.count==4096 && seed.allSatisfy{$0==0 || $0==255},"owned_input_shape")
    let lines=String(decoding:input,as:UTF8.self).split(separator:"\n");try need(lines.count==14,"case_count")
    var specs:[Spec]=[]
    for (id,line) in lines.dropFirst().enumerated(){
        let f=line.split(separator:"\t");try need(f.count==15 && Int(f[0])==id,"case_row")
        let v=f.dropFirst(2).compactMap{UInt32($0)};try need(v.count==13,"case_numeric_fields")
        try need(v[0]<=1 && v[1]<=1 && v[2]<=2 && v[3]<=1 && v[4]==0 && v[5]==1 && v[6]==15 && v[7]==0 && v[8]<=1 && v[9]<256 && v[12]==0,"case_domain")
        specs.append(Spec(id:id,name:String(f[1]),v:v))
    }
    report["case_manifest_sha256"]=sha(input);report["seed_sha256"]=sha(seed)
    report["contract"]=["width":32,"height":32,"case_count":13,"positive_cases":13,"negative_controls":"wrong expected model only; no extra native cases","framebuffer_input":"half4 color(0)",
        "no_device_buffer_fragment_side_effects":true,"shader_depth_output":false,"shader_stencil_output":false,"default_test_mode":"no early attribute, not forced hardware late execution","discard_supported":false,"control_flow_supported":false,
        "per_case_layout":"guarded before BGRA8; before RGBA8Uint stencil query; after BGRA8; RGBA32Float A; RGBA32Float B; after RGBA8Uint query",
        "guard_prefix_suffix":256,"poison_byte":205,"row_alignment":256,"diagnostic_case_bytes":68608,"visible_topology_case_bytes":43520,"visible_topology_cases":3,"model_qualification_not_inferred_from_completion":true] as [String:Any]
    let h=try Harness(source);try h.calibrate();for c in specs{try h.run(c,seed)}
    try need(commands.count==27 && observations.count==13 && drawCount<=16000,"completed_work_budget")
    report["status"]="NATIVE_OBSERVATIONS_COMPLETE";try flush()
}
var exitCode:Int32=0
do{try run()}catch{
    exitCode=3
    if let f=error as? Failure{report["failure"]=f.reason}else{let e=error as NSError;report["failure"]="native_API_error";report["error_domain"]=e.domain;report["error_code"]=e.code}
    report["status"]="FAILED_NOT_PASS";try? flush()
}
print("native_observation_exit=\(exitCode) model_qualification_is_separate=true")
exit(exitCode)
