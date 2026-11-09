// Tiny native API bridge. Own inputs only; no runtime MSL compilation/fallback.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ b: Bool, _ s: String) throws { if !b { throw Failure(message: s) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format:"%02x",$0) }.joined() }
struct Plan: Decodable { let schema,width,height,rows,cases,pitch,imageBytes,traceBytes,readBytes:Int; let offset:[Int]; let finite_only:Bool; let samplers:[String]; let formats:[Int]; let format_roles:[String] }
guard CommandLine.arguments.count == 3 && ["precise","fast"].contains(CommandLine.arguments[2]) else { exit(2) }
let root=URL(fileURLWithPath:CommandLine.arguments[1],isDirectory:true), mode=CommandLine.arguments[2]
let modeIndex:UInt32 = mode == "fast" ? 1 : 0
func read(_ n:String) throws -> Data {
    let p=root.appendingPathComponent(n)
    let a=try FileManager.default.attributesOfItem(atPath:p.path)
    let length=(a[.size] as? NSNumber)?.intValue ?? Int.max
    try need(a[.type] as? FileAttributeType == .typeRegular && length<=2_097_152,"bounded ordinary input")
    return try Data(contentsOf:p)
}
func save(_ n:String,_ d:Data) throws { try need(d.count<=2_097_152,"bounded output");try d.write(to:root.appendingPathComponent(n),options:.atomic) }
func csv(_ a:[UInt])->String {a.map(String.init).joined(separator:",")}
var report:[String:Any]=["status":"UNQUALIFIED","cpu_fallback":false]
var receipts="file\tkind\tcase\tsampler\treturn_bits\ttag\tlibrary_sha256\tinputs_sha256\tstatus\terror\tinputs_unchanged\tgpu_start\tgpu_end\tsha256\n"

func run() throws {
    try need(root.lastPathComponent==mode,"result-directory/mode identity")
    let planBytes=try read("plan.json"), p=try JSONDecoder().decode(Plan.self,from:planBytes)
    try need(p.schema==1 && p.width==5 && p.height==3 && p.rows==128 && p.cases==8 && p.pitch==256 && p.imageBytes==1280 && p.traceBytes==10688 && p.readBytes==2560 && p.offset==[0,0] && p.finite_only && p.formats==[65,115,25] && p.format_roles==["rg","control","red"] && p.samplers==["pixel-edge-linear","normalized-edge-linear"],"fixed generic plan")
    var inputData=planBytes
    let coords=try (0..<2).map {try read("coordinates-\($0).bin")}
    for d in coords {try need(d.count==p.rows*16,"coordinate length");inputData.append(d)}
    var uploads:[[Data]]=[]
    for c in 0..<p.cases {let pair=try [read("c\(c)-rg-upload.bin"),read("c\(c)-control-upload.bin"),read("c\(c)-red-upload.bin")];for d in pair {try need(d.count==p.imageBytes,"upload length");inputData.append(d)};uploads.append(pair)}
    let inputHash=sha(inputData), libData=try read("program.metallib"), libHash=sha(libData)
    guard let dev=MTLCreateSystemDefaultDevice(),let queue=dev.makeCommandQueue() else {throw Failure(message:"native Metal unavailable; no fallback")}
    let lib=try dev.makeLibrary(data:libData.withUnsafeBytes {DispatchData(bytes:$0)})
    try need(Set(lib.functionNames)==Set(["triangle","sample_f32","sample_f16","read_exact"]),"loaded entry set")
    guard let vertex=lib.makeFunction(name:"triangle"),let f32=lib.makeFunction(name:"sample_f32"),let f16=lib.makeFunction(name:"sample_f16"),let reader=lib.makeFunction(name:"read_exact") else {throw Failure(message:"missing function")}
    try need(vertex.functionType == .vertex && f32.functionType == .fragment && f16.functionType == .fragment && reader.functionType == .kernel,"entry stage")
    let readPSO=try dev.makeComputePipelineState(function:reader)
    var pipelines:[Int:MTLRenderPipelineState]=[:]
    for (bits,fn) in [(32,f32),(16,f16)] {
        let d=MTLRenderPipelineDescriptor();d.vertexFunction=vertex;d.fragmentFunction=fn
        d.rasterSampleCount=1;d.isAlphaToCoverageEnabled=false;d.isAlphaToOneEnabled=false;d.isRasterizationEnabled=true
        let a=d.colorAttachments[0]!;a.pixelFormat = .rgba8Unorm;a.isBlendingEnabled=false;a.writeMask = .all
        try need(d.rasterSampleCount==1 && !d.isAlphaToCoverageEnabled && !d.isAlphaToOneEnabled && d.isRasterizationEnabled && a.pixelFormat == .rgba8Unorm && !a.isBlendingEnabled && a.writeMask == .all,"trace-only render configuration")
        pipelines[bits]=try dev.makeRenderPipelineState(descriptor:d)
    }
    var samplers:[MTLSamplerState]=[], samplerRecords:[[String:Any]]=[]
    for norm in 0..<2 {
        let d=MTLSamplerDescriptor();d.normalizedCoordinates = norm==1
        d.sAddressMode = .clampToEdge;d.tAddressMode = .clampToEdge;d.rAddressMode = .clampToEdge
        d.minFilter = .linear;d.magFilter = .linear;d.mipFilter = .notMipmapped
        d.lodMinClamp=0;d.lodMaxClamp=0;d.maxAnisotropy=1;d.compareFunction = .never
        try need(d.normalizedCoordinates == (norm==1) && d.sAddressMode == .clampToEdge && d.tAddressMode == .clampToEdge && d.rAddressMode == .clampToEdge && d.minFilter == .linear && d.magFilter == .linear && d.mipFilter == .notMipmapped && d.lodMinClamp==0 && d.lodMaxClamp==0 && d.maxAnisotropy==1 && d.compareFunction == .never,"sampler descriptor before creation")
        guard let s=dev.makeSamplerState(descriptor:d) else {throw Failure(message:"sampler creation")};samplers.append(s)
        samplerRecords.append(["index":norm,"normalized":d.normalizedCoordinates,"min":d.minFilter.rawValue,"mag":d.magFilter.rawValue,"mip":d.mipFilter.rawValue,"s":d.sAddressMode.rawValue,"t":d.tAddressMode.rawValue,"r":d.rAddressMode.rawValue,"lod_min_bits":d.lodMinClamp.bitPattern,"lod_max_bits":d.lodMaxClamp.bitPattern,"max_anisotropy":d.maxAnisotropy,"compare":d.compareFunction.rawValue])
    }
    func buffer(_ length:Int) throws -> MTLBuffer {guard let b=dev.makeBuffer(length:length,options:.storageModeShared) else {throw Failure(message:"buffer allocation")};try need(b.length==length && b.storageMode == .shared,"actual buffer");memset(b.contents(),0xcd,length);return b}
    func snapshot(_ b:MTLBuffer)->Data {Data(bytes:b.contents(),count:b.length)}
    func copy(_ d:Data,_ b:MTLBuffer,_ offset:Int){d.withUnsafeBytes {b.contents().advanced(by:offset).copyMemory(from:$0.baseAddress!,byteCount:$0.count)}}
    let coordBuffers=try coords.map {d -> MTLBuffer in let b=try buffer(d.count+128);copy(d,b,64);return b}
    let coordBefore=coordBuffers.map(snapshot)
    var textureRecords:[[String:Any]]=[], completed=0, draws=0, reads=0
    let deadline=Date().addingTimeInterval(90)
    func finish(_ cb:MTLCommandBuffer,_ name:String,_ kind:String,_ c:Int,_ norm:Int,_ bits:Int,_ tag:UInt32,_ output:MTLBuffer,_ unchanged:()->Bool) throws {
        let signal=DispatchSemaphore(value:0);cb.addCompletedHandler {_ in signal.signal()};cb.commit()
        let remaining=min(10.0,deadline.timeIntervalSinceNow)
        try need(remaining>0 && signal.wait(timeout:.now()+remaining) == .success,"bounded GPU wait; no retry")
        let same=unchanged() && zip(coordBuffers,coordBefore).allSatisfy {snapshot($0.0)==$0.1}
        try need(cb.status == .completed && cb.error==nil && same,"completed GPU command and unchanged inputs")
        let data=snapshot(output);try save(name,data)
        receipts += [name,kind,String(c),String(norm),String(bits),String(tag),libHash,inputHash,String(cb.status.rawValue),cb.error==nil ? "0":"1",same ? "1":"0",String(cb.gpuStartTime),String(cb.gpuEndTime),sha(data)].joined(separator:"\t")+"\n"
        completed+=1
    }
    for c in 0..<p.cases {
        var textures:[MTLTexture]=[], uploadBuffers:[MTLBuffer]=[], copyBuffers:[MTLBuffer]=[]
        for (index,format) in [MTLPixelFormat.rg16Float,.rgba16Float,.r16Float].enumerated() {
            let d=MTLTextureDescriptor.texture2DDescriptor(pixelFormat:format,width:p.width,height:p.height,mipmapped:false)
            d.storageMode = .private;d.usage=[.shaderRead,.renderTarget];d.hazardTrackingMode = .tracked
            d.swizzle=MTLTextureSwizzleChannels(red:.red,green:.green,blue:.blue,alpha:.alpha)
            guard let t=dev.makeTexture(descriptor:d) else {throw Failure(message:"Private sample texture creation")}
            let sw=t.swizzle
            try need(t.pixelFormat==format && t.storageMode == .private && t.usage==[.shaderRead,.renderTarget] && t.width==5 && t.height==3 && t.depth==1 && t.mipmapLevelCount==1 && t.sampleCount==1 && t.arrayLength==1 && t.textureType == .type2D && t.buffer==nil && !t.isFramebufferOnly && t.hazardTrackingMode == .tracked && sw.red == .red && sw.green == .green && sw.blue == .blue && sw.alpha == .alpha,"actual sampled texture")
            textureRecords.append(["case":c,"role":index,"state":csv([t.pixelFormat.rawValue,t.storageMode.rawValue,t.usage.rawValue,UInt(t.width),UInt(t.height),UInt(t.depth),UInt(t.mipmapLevelCount),UInt(t.sampleCount),UInt(t.arrayLength),t.textureType.rawValue,UInt(sw.red.rawValue),UInt(sw.green.rawValue),UInt(sw.blue.rawValue),UInt(sw.alpha.rawValue)])])
            textures.append(t);let up=try buffer(p.imageBytes);copy(uploads[c][index],up,0);uploadBuffers.append(up);copyBuffers.append(try buffer(p.imageBytes))
        }
        let before=uploadBuffers.map(snapshot), size=MTLSize(width:5,height:3,depth:1), origin=MTLOrigin(x:0,y:0,z:0)
        let raw=try buffer(p.readBytes)
        guard let cb=queue.makeCommandBuffer(),let up=cb.makeBlitCommandEncoder() else {throw Failure(message:"upload command")}
        for i in 0..<3 {up.copy(from:uploadBuffers[i],sourceOffset:256,sourceBytesPerRow:256,sourceBytesPerImage:768,sourceSize:size,to:textures[i],destinationSlice:0,destinationLevel:0,destinationOrigin:origin)};up.endEncoding()
        guard let e=cb.makeComputeCommandEncoder() else {throw Failure(message:"read encoder")}
        e.setComputePipelineState(readPSO);e.setTexture(textures[0],index:0);e.setTexture(textures[0],index:1);e.setTexture(textures[1],index:2);e.setTexture(textures[1],index:3);e.setTexture(textures[2],index:4);e.setTexture(textures[2],index:5);e.setBuffer(raw,offset:64,index:0)
        let rt:UInt32=0x5000|(modeIndex<<8)|UInt32(c);var params=SIMD4<UInt32>(5,3,rt,UInt32(c));e.setBytes(&params,length:16,index:1)
        e.dispatchThreads(MTLSize(width:6,height:4,depth:1),threadsPerThreadgroup:MTLSize(width:4,height:4,depth:1));e.endEncoding();reads+=1
        guard let down=cb.makeBlitCommandEncoder() else {throw Failure(message:"readback encoder")}
        for i in 0..<3 {down.copy(from:textures[i],sourceSlice:0,sourceLevel:0,sourceOrigin:origin,sourceSize:size,to:copyBuffers[i],destinationOffset:256,destinationBytesPerRow:256,destinationBytesPerImage:768)};down.endEncoding()
        let unchanged={zip(uploadBuffers,before).allSatisfy {snapshot($0.0)==$0.1}}
        try finish(cb,"c\(c)-read.bin","typed-read",c,-1,0,rt,raw,unchanged)
        for (i,suffix) in ["rg","control","red"].enumerated() {try save("c\(c)-\(suffix)-copy.bin",snapshot(copyBuffers[i]))}
        let captures=(0..<2).flatMap {s in [16,32].map {(s,$0,false)}} + (c==0 ? [(0,32,true)] : [])
        for (norm,bits,negative) in captures {
            let trace=try buffer(p.traceBytes)
            let td=MTLTextureDescriptor.texture2DDescriptor(pixelFormat:.rgba8Unorm,width:3,height:p.rows+2,mipmapped:false)
            td.storageMode = .private;td.usage = .renderTarget
            guard let target=dev.makeTexture(descriptor:td),let command=queue.makeCommandBuffer() else {throw Failure(message:"trace render target/command")}
            let pass=MTLRenderPassDescriptor();pass.colorAttachments[0].texture=target;pass.colorAttachments[0].loadAction = .dontCare;pass.colorAttachments[0].storeAction = .dontCare
            guard let enc=command.makeRenderCommandEncoder(descriptor:pass),let pipeline=pipelines[bits] else {throw Failure(message:"render encoder")}
            enc.setRenderPipelineState(pipeline);enc.setCullMode(.none);enc.setTriangleFillMode(.fill)
            enc.setViewport(MTLViewport(originX:0,originY:0,width:3,height:Double(p.rows+2),znear:0,zfar:1));enc.setScissorRect(MTLScissorRect(x:1,y:1,width:1,height:p.rows))
            enc.setFragmentTexture(textures[0],index:0);enc.setFragmentTexture(textures[1],index:1);enc.setFragmentTexture(textures[2],index:2);enc.setFragmentSamplerState(samplers[norm],index:0)
            enc.setFragmentBuffer(coordBuffers[norm],offset:64,index:0);enc.setFragmentBuffer(trace,offset:64,index:1)
            let tag=(modeIndex<<12)|(UInt32(c)<<4)|(UInt32(norm)<<2)|(bits==16 ? 1:2)
            var pp=SIMD4<UInt32>(UInt32(p.rows),tag,negative ? 0:UInt32.max,UInt32(c));enc.setFragmentBytes(&pp,length:16,index:2)
            enc.drawPrimitives(type:.triangle,vertexStart:0,vertexCount:3);enc.endEncoding();draws+=1
            try finish(command,negative ? "negative-skip.bin":"c\(c)-s\(norm)-f\(bits).bin",negative ? "negative-skip":"sample",c,norm,bits,tag,trace,unchanged)
        }
    }
    try need(completed==41 && draws==33 && reads==8,"exact command accounting")
    report=["status":"NATIVE_BYTES_CAPTURED_PROFILE_UNSELECTED","cpu_fallback":false,"device":dev.name,"os":ProcessInfo.processInfo.operatingSystemVersionString,"unified_memory":dev.hasUnifiedMemory,"mode_label":mode,"loaded_sha256":libHash,"inputs_sha256":inputHash,"command_buffers":completed,"draws":draws,"typed_read_dispatches":reads,"positive_sample_records":4096,"sample_values_three_formats":12288,"native_negative_executed_records":127,"samplers":samplerRecords,"textures":textureRecords,"sampler_scope":"ordinary implicit-LOD fragment sample with runtime descriptor queried immediately before creation; no bias/offset overload; NOT native constexpr-sampler introspection","trace_scope":"raw typed sampler results before any render-target conversion; color attachment is not an oracle","guard_scope":"host upload/copy/read/trace guards, NOT opaque Private allocation padding","math_profile":"inspect exact linked AIR; mode name alone is not Preserve proof"]
}
var code:Int32=0
do {try run()} catch {report["status"]="UNQUALIFIED_NATIVE_FAILURE";report["error"]="\(error)";code=3}
do {try save("gpu.tsv",Data(receipts.utf8));let d=try JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]);try save("gpu.json",d);FileHandle.standardOutput.write(d)} catch {FileHandle.standardError.write(Data("receipt failure: \(error)\n".utf8));code=3}
exit(code)
