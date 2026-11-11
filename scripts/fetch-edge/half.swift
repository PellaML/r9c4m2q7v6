// Minimal adaptation of the public fetch-edge/order.swift acquisition protocol.
// Rust owns the synthetic schedule/inputs and checks. No private code or metadata.
import Foundation
import Metal
import Dispatch

struct Failure: Error {}
func need(_ b: Bool) throws { if !b { throw Failure() } }
func bytes<T>(_ a: [T]) -> Data { a.withUnsafeBytes { Data($0) } }
func json(_ a: [String:Int]) throws -> Data {
    try JSONSerialization.data(withJSONObject:a, options:[.sortedKeys])
}
let N=4096, G=256, recordBytes=96
let header="case\tformat\tseed\tsplit\tdraw\trx\try\trw\trh\tsx\tsy\tsw\tsh\tdiagonal\twinding\tmutation\tphase\tscale_bits\tsnapshot"
struct Draw {
    let v: [Int]
    var points: [SIMD2<Float>] {
        let x=Float(v[5]), y=Float(v[6]), r=x+Float(v[7]), b=y+Float(v[8])
        let tl=SIMD2(x,y), tr=SIMD2(r,y), bl=SIMD2(x,b), br=SIMD2(r,b)
        if v[13]==0 { return v[14]==0 ? [tl,bl,tr,br] : [tl,tr,bl,br] }
        return v[14]==0 ? [tr,tl,br,bl] : [tr,br,tl,bl]
    }
    func parameters() -> [UInt32] {
        [v[0],v[4],v[15],v[16],v[4]*2*N,v[1],v[13],v[14],
         v[5],v[6],v[7],v[8],v[9],v[10],v[11],v[12],v[17],0,0,0].map(UInt32.init)
    }
}
var phase=0, completedCommands=0, completedCases=0, totalBytes=0
var receipts=Data()
let root=URL(fileURLWithPath:CommandLine.arguments.count==3 ? CommandLine.arguments[2] : ".")
func save(_ name: String, _ data: Data) throws {
    totalBytes += data.count
    try need(totalBytes <= 4*1024*1024-128*1024 && data.count <= 512*1024)
    try data.write(to:root.appendingPathComponent(name), options:.withoutOverwriting)
}
func read(_ name: String, _ cap: Int) throws -> Data {
    let url=root.appendingPathComponent(name)
    let kind=try url.resourceValues(forKeys:[.isRegularFileKey,.isSymbolicLinkKey,.fileSizeKey])
    try need(kind.isRegularFile==true && kind.isSymbolicLink==false && (kind.fileSize ?? cap+1)<=cap)
    let data=try Data(contentsOf:url); totalBytes += data.count; return data
}
func word(_ data: Data, _ at: Int) -> UInt32 {
    UInt32(data[at]) | UInt32(data[at+1])<<8 | UInt32(data[at+2])<<16 | UInt32(data[at+3])<<24
}
func run() throws {
    try need(CommandLine.arguments.count==3 && ProcessInfo.processInfo.operatingSystemVersion.majorVersion==26)
    #if arch(arm64)
    let arm=1
    #else
    let arm=0
    #endif
    try need(arm==1 && MemoryLayout<UInt32>.size==4 && MemoryLayout<SIMD2<Float>>.stride==8)
    let inputs=try read("inputs.bin",3*N*8+N*4)
    let snapshots=try read("snapshots.bin",128*1024)
    let round=try read("rounding.bin",128)
    let table=try read("cases.tsv",16384)
    try need(inputs.count==3*N*8+N*4 && round.count==128 && snapshots.count>0 && snapshots.count%(N*2)==0)
    guard let text=String(data:table,encoding:.utf8) else {throw Failure()}
    let lines=text.split(separator:"\n").map(String.init)
    try need(lines.first==header && lines.count==31)
    var cases=[[Draw]](repeating:[],count:24)
    for line in lines.dropFirst() {
        let columns=line.split(separator:"\t",omittingEmptySubsequences:false)
        let v=columns.compactMap {Int($0)}
        try need(v.count==19 && v.allSatisfy{$0>=0 && $0<=65535})
        try need(v[0]<24 && v[1]<2 && v[2]<4 && v[3]<2 && v[4]<3 && v[4]==cases[v[0]].count)
        try need(v[13]<2 && v[14]<2 && v[15]<2 && v[16]<3 && [0x3800,0x3c00,0x3e00].contains(v[17]))
        try need(v[5]+v[7]<=64 && v[6]+v[8]<=64 && v[9]+v[11]<=64 && v[10]+v[12]<=64)
        try need(v[7]>0 && v[8]>0 && v[11]>0 && v[12]>0 && (v[18]+1)*N*2<=snapshots.count)
        try need(v[1]==0 ? v[2]<3 : v[2]==3)
        if let first=cases[v[0]].first {try need(first.v[0..<4].elementsEqual(v[0..<4]))}
        cases[v[0]].append(Draw(v:v))
    }
    try need(cases.allSatisfy{!$0.isEmpty && $0.count<=3})
    guard let dev=MTLCreateSystemDefaultDevice(), let queue=dev.makeCommandQueue() else {throw Failure()}
    print("REFERENCE_DEVICE \(dev.name)")
    print("REFERENCE_OS \(ProcessInfo.processInfo.operatingSystemVersionString)")
    func buffer(_ data: Data) throws -> MTLBuffer {
        try data.withUnsafeBytes { p in
            guard let b=dev.makeBuffer(bytes:p.baseAddress!,length:p.count,options:.storageModeShared) else {throw Failure()}
            return b
        }
    }
    let snapshotBuffer=try buffer(snapshots), roundingBuffer=try buffer(round)
    let opt=MTLCompileOptions()
    let defaultMode=Int(opt.mathMode.rawValue), defaultFunctions=Int(opt.mathFloatingPointFunctions.rawValue)
    let defaultFast=opt.fastMathEnabled ? 1 : 0
    opt.languageVersion = .version2_4
    opt.fastMathEnabled = true
    opt.mathMode = .fast
    opt.mathFloatingPointFunctions = .fast
    // No public denorm override is requested. Runtime half and binary32 probes
    // record behavior; these API settings do NOT identify any private IR mode.
    try save("environment.json",json([
        "schema":1,"os_major":26,"arm_sixty_four":arm,"language_two_four":opt.languageVersion == .version2_4 ? 1:0,
        "default_math_mode_raw":defaultMode,"default_math_functions_raw":defaultFunctions,"default_fast_math":defaultFast,
        "effective_math_mode_raw":Int(opt.mathMode.rawValue),"effective_math_functions_raw":Int(opt.mathFloatingPointFunctions.rawValue),
        "effective_fast_math":opt.fastMathEnabled ? 1:0,"effective_mode_fast":opt.mathMode == .fast ? 1:0,
        "effective_functions_fast":opt.mathFloatingPointFunctions == .fast ? 1:0,
        "denorm_override_requested":0,"ir_math_equivalence_claimed":0,"scalar_half_primary":1,"helper_values_observed":0,
        "record_bytes":recordBytes,"parameter_bytes":80,"unified_memory":dev.hasUnifiedMemory ? 1:0,
        "apple_seven":dev.supportsFamily(.apple7) ? 1:0,"mac_two":dev.supportsFamily(.mac2) ? 1:0
    ]))
    phase=1
    let source=try String(contentsOfFile:CommandLine.arguments[1],encoding:.utf8)
    try need(source.utf8.count<=32768)
    let lib=try dev.makeLibrary(source:source,options:opt)
    var pipelines=[MTLRenderPipelineState](), descriptors=[MTLRenderPipelineDescriptor]()
    for format in [MTLPixelFormat.rgba16Float,.bgra8Unorm] {
        let pd=MTLRenderPipelineDescriptor()
        pd.vertexFunction=lib.makeFunction(name:"half_vertex"); pd.fragmentFunction=lib.makeFunction(name:"half_fetch")
        try need(pd.vertexFunction != nil && pd.fragmentFunction != nil)
        pd.rasterSampleCount=1
        pd.colorAttachments[0].pixelFormat=format; pd.colorAttachments[1].pixelFormat = .rg16Float
        for i in 0..<2 {pd.colorAttachments[i].isBlendingEnabled=false}
        pd.colorAttachments[0].writeMask = .all; pd.colorAttachments[1].writeMask = [.red,.green]
        pipelines.append(try dev.makeRenderPipelineState(descriptor:pd)); descriptors.append(pd)
    }
    let started=DispatchTime.now().uptimeNanoseconds
    func finish(_ cb: MTLCommandBuffer) throws {
        try need(completedCommands<26 && DispatchTime.now().uptimeNanoseconds-started<280_000_000_000)
        let done=DispatchSemaphore(value:0)
        cb.addCompletedHandler { _ in done.signal() }
        cb.commit() // exactly once; no retries, replacement queues, or fallback
        try need(done.wait(timeout:.now()+10) == .success)
        try need(cb.status == .completed && cb.error == nil)
        completedCommands += 1
    }
    func texture(_ f: MTLPixelFormat, _ storage: MTLStorageMode) throws -> MTLTexture {
        let td=MTLTextureDescriptor.texture2DDescriptor(pixelFormat:f,width:64,height:64,mipmapped:false)
        td.storageMode=storage; td.usage=[.renderTarget,.shaderRead]
        guard let t=dev.makeTexture(descriptor:td) else {throw Failure()}; return t
    }
    for (id,draws) in cases.enumerated() {
        try autoreleasepool {
            phase=2
            let c=draws[0].v, format=c[1], seed=c[2], split=c[3]==1
            let bpp=format==0 ? 8:4, pitch=64*bpp, plen=2*G+N*bpp
            let color=try texture(format==0 ? .rgba16Float:.bgra8Unorm,.shared)
            let discarded=try texture(.rg16Float,.private)
            let begin=seed<3 ? seed*N*8:3*N*8
            let initial=inputs.subdata(in:begin..<begin+N*bpp)
            initial.withUnsafeBytes {p in color.replace(region:MTLRegionMake2D(0,0,64,64),mipmapLevel:0,withBytes:p.baseAddress!,bytesPerRow:pitch)}
            let traceLength=draws.count*2*N*recordBytes
            var traceData=Data(repeating:0xa5,count:traceLength+2*G)
            traceData.resetBytes(in:G..<G+traceLength)
            let trace=try buffer(traceData), readback=try buffer(Data(repeating:0xa5,count:2*plen))
            let vertexBuffers=try draws.map {try buffer(bytes($0.points))}
            let groups=split ? draws.indices.map{[$0]}:[Array(draws.indices)]
            let beforeCommands=completedCommands
            var primaryLoad=true, primaryStore=true, secondaryClear=true, secondaryDiscard=true
            func copy(_ cb: MTLCommandBuffer, _ offset: Int) throws {
                guard let blit=cb.makeBlitCommandEncoder() else {throw Failure()}
                blit.copy(from:color,sourceSlice:0,sourceLevel:0,sourceOrigin:MTLOrigin(x:0,y:0,z:0),
                    sourceSize:MTLSize(width:64,height:64,depth:1),to:readback,destinationOffset:offset,
                    destinationBytesPerRow:pitch,destinationBytesPerImage:N*bpp)
                blit.endEncoding()
            }
            for (gi,group) in groups.enumerated() {
                guard let cb=queue.makeCommandBuffer() else {throw Failure()}
                if gi==0 {try copy(cb,G)} // actual native upload roundtrip, BEFORE any draw
                let rp=MTLRenderPassDescriptor()
                rp.colorAttachments[0].texture=color; rp.colorAttachments[0].loadAction = .load; rp.colorAttachments[0].storeAction = .store
                rp.colorAttachments[1].texture=discarded; rp.colorAttachments[1].loadAction = .clear; rp.colorAttachments[1].storeAction = .dontCare
                rp.colorAttachments[1].clearColor=MTLClearColor(red:0.25,green:0.5,blue:0,alpha:0)
                primaryLoad = primaryLoad && rp.colorAttachments[0].loadAction == .load
                primaryStore = primaryStore && rp.colorAttachments[0].storeAction == .store
                secondaryClear = secondaryClear && rp.colorAttachments[1].loadAction == .clear
                secondaryDiscard = secondaryDiscard && rp.colorAttachments[1].storeAction == .dontCare
                guard let e=cb.makeRenderCommandEncoder(descriptor:rp) else {throw Failure()}
                e.setRenderPipelineState(pipelines[format]); e.setCullMode(.none)
                e.setViewport(MTLViewport(originX:0,originY:0,width:64,height:64,znear:0,zfar:1))
                e.setFragmentBuffer(trace,offset:G,index:1); e.setFragmentBuffer(roundingBuffer,offset:0,index:3)
                for di in group {
                    let d=draws[di], v=d.v
                    let p=d.parameters(); try need(p.count*4==80)
                    e.setVertexBuffer(vertexBuffers[di],offset:0,index:0)
                    p.withUnsafeBytes {e.setFragmentBytes($0.baseAddress!,length:$0.count,index:0)}
                    e.setFragmentBuffer(snapshotBuffer,offset:v[18]*N*2,index:2)
                    e.setScissorRect(MTLScissorRect(x:v[9],y:v[10],width:v[11],height:v[12]))
                    e.drawPrimitives(type:.triangleStrip,vertexStart:0,vertexCount:4)
                }
                e.endEncoding()
                if gi==groups.count-1 {try copy(cb,plen+G)} // NEVER read the discarded attachment
                try finish(cb)
            }
            phase=3
            let rawTrace=Data(bytes:trace.contents(),count:trace.length)
            try need(rawTrace.prefix(G).allSatisfy{$0==0xa5} && rawTrace.suffix(G).allSatisfy{$0==0xa5})
            var compact=Data()
            for i in 0..<draws.count*2*N {
                let at=G+i*recordBytes
                let row=rawTrace.subdata(in:at..<at+recordBytes)
                if word(row,0)==0 {try need(row.allSatisfy{$0==0})} else {compact.append(row)}
            }
            try need(compact.count>0 && compact.count%recordBytes==0)
            let records=compact.count/recordBytes
            var raw=bytes([UInt32(0x48464f31),1,UInt32(id),UInt32(format),UInt32(draws.count),UInt32(records),UInt32(plen),UInt32(recordBytes)])
            raw.append(Data(bytes:readback.contents(),count:readback.length))
            raw.append(rawTrace.prefix(G)); raw.append(rawTrace.suffix(G)); raw.append(compact)
            try save(String(format:"case%02d.bin",id),raw)
            let pd=descriptors[format], commands=completedCommands-beforeCommands
            let receipt=try json([
                "id":id,"format":format,"seed":seed,"draws":draws.count,"commands":commands,"completed":commands,"errors":0,
                "width":color.width,"height":color.height,"pitch":pitch,"samples":color.sampleCount,"shared":color.storageMode == .shared ? 1:0,
                "primary_format_matches":(pd.colorAttachments[0].pixelFormat==color.pixelFormat && color.pixelFormat==(format==0 ? .rgba16Float:.bgra8Unorm)) ? 1:0,
                "primary_load":primaryLoad ? 1:0,"primary_store":primaryStore ? 1:0,"secondary_rg_half":discarded.pixelFormat == .rg16Float ? 1:0,
                "secondary_clear":secondaryClear ? 1:0,"secondary_discard":secondaryDiscard ? 1:0,
                "primary_write_all":pd.colorAttachments[0].writeMask == .all ? 1:0,
                "secondary_write_rg":pd.colorAttachments[1].writeMask == [.red,.green] ? 1:0,
                "blend_disabled":(!pd.colorAttachments[0].isBlendingEnabled && !pd.colorAttachments[1].isBlendingEnabled) ? 1:0,
                "cull_none":1,"reciprocal_w_one":1,"trace_guards":1,"records":records,"bytes":raw.count
            ])
            receipts.append(receipt); receipts.append(10); completedCases += 1
        }
    }
    try need(completedCases==24 && completedCommands==26)
    try save("completion.json",json(["schema":1,"cases":completedCases,"draws":30,"commands":completedCommands,"status":1]))
}
var code:Int32=0
do {try run()} catch {
    code=3
    print("REFERENCE_FAILURE \(error)")
    try? save("failure.json",json(["phase":phase,"completed_cases":completedCases,"completed_commands":completedCommands,"status":0]))
}
do {try save("native-cases.jsonl",receipts)} catch {code=3}
print(code==0 ? "NATIVE_CAPTURED_NOT_YET_CHECKED" : "NATIVE_NOT_PROOF")
exit(code)
