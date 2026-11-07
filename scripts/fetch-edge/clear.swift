// Newly authored synthetic data and geometry; no external files except our MSL.
import Foundation
import Metal
import CryptoKit
struct Failure: Error { let message: String }
func need(_ b: Bool, _ s: String) throws { if !b { throw Failure(message:s) } }
func bytes<T>(_ v:[T])->Data { v.withUnsafeBytes{Data($0)} }
func sha(_ d:Data)->String { SHA256.hash(data:d).map{String(format:"%02x",$0)}.joined() }
let W=32, H=32, N=1024
struct Draw {
    let points:[SIMD2<Float>]; let strip:Bool; let scissor:[Int]; let mode:UInt32; let marker:Float
    var prims:Int { strip ? points.count-2 : points.count/3 }
}
struct Case {
    var name:String; let seed:Int; let scale:Float; let bias:Float; let draws:[Draw]
    var storage:String="shared"; var math:String="safe"; var decl:String="float"; var separate:Bool=false; var loadClear:Bool=false
}
struct Params { var scale:Float; var bias:Float; var marker:Float; var mode:UInt32; var base:UInt32; var width:UInt32=32; var height:UInt32=32; var unused:UInt32=0 }
let all=[0,0,32,32], small=[0,0,14,13], shifted=[3,5,18,15]
let full:[SIMD2<Float>]=[SIMD2(-32,-32),SIMD2(96,-32),SIMD2(-32,96)]
let rect:[SIMD2<Float>]=[SIMD2(0,0),SIMD2(0,13),SIMD2(14,0),SIMD2(14,13)]
let offsetRect:[SIMD2<Float>]=[SIMD2(3,5),SIMD2(3,20),SIMD2(21,5),SIMD2(21,20)]
let fractionRect:[SIMD2<Float>]=[SIMD2(5.25,3.75),SIMD2(5.25,24.25),SIMD2(26.75,3.75),SIMD2(26.75,24.25)]
func triangles(_ r:[SIMD2<Float>])->[[SIMD2<Float>]] { [[r[0],r[1],r[2]],[r[2],r[1],r[3]]] }
func draw(_ p:[SIMD2<Float>],_ strip:Bool=false,_ sc:[Int]=all,_ mode:UInt32=0,_ marker:Float=0.1875)->Draw {
    Draw(points:p,strip:strip,scissor:sc,mode:mode,marker:marker)
}
let ts=triangles(rect)
var cases:[Case]=[]
func add(_ name:String,_ ds:[Draw],_ clear:Bool=true,_ seed:Int=0) {
    var c=Case(name:name,seed:seed,scale:1.234567,bias:-0.13579,draws:ds);c.loadClear=clear;cases.append(c)
}
add("clear-echo",[draw(full)])
add("clear-small-read",[draw(rect,true,small)])
add("clear-small-six-effect",(0..<6).map{_ in draw(rect,true,small,3)})
add("clear-small-six-invert",(0..<6).map{_ in draw(rect,true,small,1)})
add("clear-full-invert-then-small",[draw(full,false,all,1),draw(rect,true,small,3)])
add("clear-full-effect-then-small",[draw(full,false,all,3),draw(rect,true,small,3)])
add("clear-full-marker-then-small",[draw(full,false,all,2),draw(rect,true,small,3)])
add("clear-offset-six",(0..<6).map{_ in draw(offsetRect,true,shifted,1)})
add("clear-fractional-six",(0..<6).map{_ in draw(fractionRect,true,all,1)})
add("clear-pair-draws",[draw(ts[0],false,small,2),draw(ts[1],false,small,3)])
add("load-triangle-predecessor-strip",[draw(ts[0],false,small,2),draw(rect,true,small,3)],false)
add("clear-triangle-predecessor-strip",[draw(ts[0],false,small,2),draw(rect,true,small,3)])
add("load-half-screen-predecessor-strip",[draw(full,false,[0,0,7,32],2),draw(rect,true,small,3)],false)
add("load-offset-six",(0..<6).map{_ in draw(offsetRect,true,shifted,1)},false)
add("load-fractional-six",(0..<6).map{_ in draw(fractionRect,true,all,1)},false)
add("clear-small-read-other-seed",[draw(rect,true,small)],true,1)
for name in ["clear-small-six-effect","clear-full-marker-then-small"] {
    let b=cases.first{$0.name==name}!
    var c=b;c.storage="private";c.name += "-private";cases.append(c)
    c=b;c.math="default";c.name += "-default";cases.append(c)
}
var sep=cases.first{$0.name=="clear-pair-draws"}!;sep.separate=true;sep.name += "-separate-pass";cases.append(sep)
func seedBytes(_ seed:Int)->Data {
    var a=[UInt8](repeating:0,count:N*4)
    let specials:[UInt8]=[0,1,2,63,64,127,128,129,191,192,253,254,255]
    for y in 0..<H { for x in 0..<W {
        let i=(y*W+x)*4
        a[i]=UInt8((x*13+y*29+7)&255); a[i+1]=UInt8((x*37+y*5+19)&255); a[i+2]=UInt8((x*3+y*43+101)&255)
        switch seed {
        case 0: a[i+3]=UInt8(((x*73+y*151+x*y*19)^(x<<2)^(y<<4))&255)
        case 1: a[i+3]=specials[(x*7+y*11+x*y)%specials.count]
        default: a[i+3]=217
        }
    } }
    return Data(a)
}
func run() throws -> [String:Any] {
    try need(CommandLine.arguments.count==3,"probe.metal outdir")
    let root=URL(fileURLWithPath:CommandLine.arguments[2])
    guard let dev=MTLCreateSystemDefaultDevice(), let queue=dev.makeCommandQueue() else { throw Failure(message:"UNAVAILABLE device/queue") }
    let source=try String(contentsOfFile:CommandLine.arguments[1],encoding:.utf8)
    var pipelines:[String:MTLRenderPipelineState]=[:]
    for mode in ["safe","default"] {
        let opt=MTLCompileOptions()
        if mode=="safe" { if #available(macOS 15.0,*) { opt.mathMode = .safe } else { opt.fastMathEnabled=false } }
        let lib=try dev.makeLibrary(source:source,options:opt)
        for decl in ["float","half"] {
            let pd=MTLRenderPipelineDescriptor(); pd.vertexFunction=lib.makeFunction(name:"vertex_main"); pd.fragmentFunction=lib.makeFunction(name:"fetch_"+decl)
            for (i,f) in [MTLPixelFormat.bgra8Unorm,.rgba32Float,.rgba32Float].enumerated() {
                pd.colorAttachments[i].pixelFormat=f; pd.colorAttachments[i].isBlendingEnabled=false
            }
            pipelines[mode+"-"+decl]=try dev.makeRenderPipelineState(descriptor:pd)
        }
    }
    func buf(_ data:Data) throws -> MTLBuffer {
        try data.withUnsafeBytes { p throws -> MTLBuffer in
            guard let b=dev.makeBuffer(bytes:p.baseAddress!,length:p.count,options:.storageModeShared) else { throw Failure(message:"buffer") }; return b
        }
    }
    func finish(_ cb:MTLCommandBuffer) throws {
        cb.commit(); cb.waitUntilCompleted(); try need(cb.status == .completed && cb.error == nil,"command status \(cb.status.rawValue) \(String(describing:cb.error))")
    }
    func makeTex(_ format:MTLPixelFormat,_ storage:MTLStorageMode)->MTLTexture? {
        let d=MTLTextureDescriptor.texture2DDescriptor(pixelFormat:format,width:W,height:H,mipmapped:false)
        d.storageMode=storage; d.usage=[.renderTarget,.shaderRead]; return dev.makeTexture(descriptor:d)
    }
    var manifest:[[String:Any]]=[], fileHashes:[String:String]=[:], byHash:[String:String]=[:]
    var totalBytes=0
    func save(_ name:String,_ data:Data) throws -> String {
        let hash=sha(data); if let old=byHash[hash] {return old}
        totalBytes += data.count; try need(totalBytes<=23*1024*1024,"artifact data budget exceeded")
        try data.write(to:root.appendingPathComponent(name)); fileHashes[name]=hash; byHash[hash]=name; return name
    }
    for seed in 0..<3 { _=try save("seed\(seed)-bgra8.bin",seedBytes(seed)) }
    for (ci,c) in cases.enumerated() {
        let storage:MTLStorageMode=c.storage=="private" ? .private : .shared
        guard let color=makeTex(.bgra8Unorm,storage), let t1=makeTex(.rgba32Float,storage), let t2=makeTex(.rgba32Float,storage) else {throw Failure(message:"texture allocation")}
        let initial=seedBytes(c.seed)
        if storage == .shared { initial.withUnsafeBytes {color.replace(region:MTLRegionMake2D(0,0,W,H),mipmapLevel:0,withBytes:$0.baseAddress!,bytesPerRow:W*4)} }
        else {
            let up=try buf(initial)
            guard let cb=queue.makeCommandBuffer(),let blit=cb.makeBlitCommandEncoder() else {throw Failure(message:"upload")}
            blit.copy(from:up,sourceOffset:0,sourceBytesPerRow:W*4,sourceBytesPerImage:N*4,sourceSize:MTLSize(width:W,height:H,depth:1),to:color,destinationSlice:0,destinationLevel:0,destinationOrigin:MTLOrigin(x:0,y:0,z:0))
            blit.endEncoding(); try finish(cb)
        }
        let prims=c.draws.reduce(0){$0+$1.prims}
        let trace=try buf(Data(repeating:0,count:prims*N*24*4))
        var drawMeta:[[String:Any]]=[], allVertexBuffers:[MTLBuffer]=[]
        for d in c.draws { allVertexBuffers.append(try buf(bytes(d.points))) }
        var base=0
        var groups:[[Int]]=[]
        if c.separate { groups=c.draws.indices.map{[$0]} } else { groups=[Array(c.draws.indices)] }
        for (gi,group) in groups.enumerated() {
            let rp=MTLRenderPassDescriptor()
            for (i,t) in [color,t1,t2].enumerated() {
                rp.colorAttachments[i].texture=t; rp.colorAttachments[i].loadAction = i==0 ? (gi==0 && c.loadClear ? .clear : .load) : (gi>0 ? .load : .clear)
                rp.colorAttachments[i].storeAction = .store
                rp.colorAttachments[i].clearColor = i==0 ? MTLClearColor(red:0.125,green:0.25,blue:0.75,alpha:0.625) : MTLClearColor(red:-999,green:-999,blue:-999,alpha:-999)
            }
            guard let cb=queue.makeCommandBuffer(),let e=cb.makeRenderCommandEncoder(descriptor:rp) else {throw Failure(message:"encoder")}
            e.setRenderPipelineState(pipelines[c.math+"-"+c.decl]!); e.setCullMode(.none)
            e.setViewport(MTLViewport(originX:0,originY:0,width:32,height:32,znear:0,zfar:1))
            for di in group {
                let d=c.draws[di]
                var p=Params(scale:c.scale,bias:c.bias,marker:d.marker,mode:d.mode,base:UInt32(base))
                e.setVertexBuffer(allVertexBuffers[di],offset:0,index:0)
                e.setFragmentBytes(&p,length:MemoryLayout<Params>.size,index:0); e.setFragmentBuffer(trace,offset:0,index:1)
                e.setScissorRect(MTLScissorRect(x:d.scissor[0],y:d.scissor[1],width:d.scissor[2],height:d.scissor[3]))
                e.drawPrimitives(type:d.strip ? .triangleStrip : .triangle,vertexStart:0,vertexCount:d.points.count)
                drawMeta.append(["draw":di,"trace_base_records":base,"primitives":d.prims,"strip":d.strip,
                                 "points":d.points.map{[$0.x,$0.y]},"scissor":d.scissor,"write_mode":d.mode,"marker":d.marker])
                base += d.prims*N
            }
            e.endEncoding(); try finish(cb)
        }
        let readback=try buf(Data(repeating:0,count:N*(4+16+16)))
        guard let cb=queue.makeCommandBuffer(),let blit=cb.makeBlitCommandEncoder() else {throw Failure(message:"readback")}
        var offset=0
        for (t,bpp) in [(color,4),(t1,16),(t2,16)] {
            blit.copy(from:t,sourceSlice:0,sourceLevel:0,sourceOrigin:MTLOrigin(x:0,y:0,z:0),sourceSize:MTLSize(width:W,height:H,depth:1),to:readback,destinationOffset:offset,destinationBytesPerRow:W*bpp,destinationBytesPerImage:N*bpp)
            offset += N*bpp
        }
        blit.endEncoding(); try finish(cb)
        var raw=Data(bytes:readback.contents(),count:readback.length)
        raw.append(Data(bytes:trace.contents(),count:trace.length))
        let name=String(format:"c%03d.bin",ci), file=try save(name,raw)
        manifest.append(["case":ci,"name":c.name,"seed":c.seed,"scale_bits":c.scale.bitPattern,"bias_bits":c.bias.bitPattern,
                         "storage":c.storage,"math":c.math,"input_type":c.decl,"separate_pass":c.separate,"draws":drawMeta,"load_clear":c.loadClear,"clear_rgba":[0.125,0.25,0.75,0.625],
                         "file":file,"sha256":sha(raw),"bytes":raw.count])
    }
    var device:[String:Any]=["name":dev.name,"hasUnifiedMemory":dev.hasUnifiedMemory,"registryID":dev.registryID,
                            "mac2":dev.supportsFamily(.mac2),"apple7":dev.supportsFamily(.apple7)]
    if #available(macOS 14.0,*) {device["architecture"]=dev.architecture.name}
    return ["status":"NATIVE_OBSERVATIONS_COMPLETE","device":device,"os":ProcessInfo.processInfo.operatingSystemVersionString,
            "case_count":cases.count,"cases":manifest,"files_sha256":fileHashes,"data_bytes":totalBytes,
            "layout":"per case: BGRA8 final 4096 bytes; rgba32Float target1 [h,dxH,dyH,fwH] 16384 bytes; target2 [aH,fma,dxF,dyF] 16384 bytes; then per-primitive 1024 trace records of 24 u32",
            "trace_fields":["stamp","primitive","prior_alpha","half_alpha","float_fma","half_fma","half_dx","half_dy","half_fw","half_abs_dx_plus_abs_dy","float_dx_of_half","float_dy_of_half","float_fw_of_half","half_effect","written_alpha","analytic_half","analytic_dx","analytic_dy","analytic_fw","position_x","position_y","prior_r","prior_g","prior_b"]]
}
let root=CommandLine.arguments.count>2 ? CommandLine.arguments[2] : "."
var result:[String:Any]; var code:Int32=0
do { result=try run() } catch {result=["status":"FAILED_NOT_PASS","error":"\(error)"]; code=3}
let data=try! JSONSerialization.data(withJSONObject:result,options:[.sortedKeys,.prettyPrinted])
try! data.write(to:URL(fileURLWithPath:root).appendingPathComponent("summary.json"))
FileHandle.standardOutput.write(data.prefix(2000)); exit(code)
