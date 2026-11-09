// Tiny native-only bridge. No runtime source compilation or CPU result fallback.
import Foundation
import Metal
import CryptoKit
import Dispatch

struct Failure: Error { let reason: String }
func need(_ ok: Bool, _ reason: String) throws { if !ok { throw Failure(reason: reason) } }
func sha(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
var receipt = "mode\tloaded_sha256\tstatus\terror\tinputs_unchanged\n"
var report: [String: Any] = ["schema": 1, "status": "UNQUALIFIED_NOT_COMPLETE", "cpu_fallback": false]
guard CommandLine.arguments.count == 2 else { exit(2) }
let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
func read(_ name: String) throws -> Data {
    let data = try Data(contentsOf: root.appendingPathComponent(name))
    try need(data.count <= 1_048_576, "input byte bound")
    return data
}
func save(_ name: String, _ data: Data) throws {
    try need(data.count <= 1_048_576, "output byte bound")
    try data.write(to: root.appendingPathComponent(name), options: .withoutOverwriting)
}
func run() throws {
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little endian required")
    let halfData = try read("half.bin"), vertexData = try read("vertices.bin")
    try need(halfData.count == 40 && vertexData.count == 192, "fixed input sizes")
    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else {
        throw Failure(reason: "native Metal device unavailable")
    }
    report["device"] = dev.name // No registry identifier, serial, hostname, or address.
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    report["unified_memory"] = dev.hasUnifiedMemory
    report["layout"] = "20 scalar rows; two 4x4 unequal-W cases; two RGBA32Uint targets; sample count 1"
    func buffer(_ length: Int) throws -> MTLBuffer {
        guard let b = dev.makeBuffer(length: length, options: .storageModeShared) else { throw Failure(reason: "buffer") }
        memset(b.contents(), 0xcd, b.length)
        return b
    }
    func upload(_ data: Data) throws -> MTLBuffer {
        let b = try buffer(data.count + 128)
        data.withUnsafeBytes { b.contents().advanced(by: 64).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
        return b
    }
    func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    let halfInput = try upload(halfData), vertices = try upload(vertexData)
    let halfBefore = snapshot(halfInput), verticesBefore = snapshot(vertices)
    for (modeIndex, mode) in ["precise", "fast"].enumerated() {
        let bytes = try read(mode + ".metallib")
        try need(Array(bytes.prefix(4)) == [0x4d, 0x54, 0x4c, 0x42], "metallib signature")
        let library = try dev.makeLibrary(data: bytes.withUnsafeBytes { DispatchData(bytes: $0) })
        try need(Set(library.functionNames) == Set(["scalar_half_ushort", "large_vertex", "large_fragment"]), "entry set")
        guard let kernel = library.makeFunction(name: "scalar_half_ushort"),
              let vertex = library.makeFunction(name: "large_vertex"),
              let fragment = library.makeFunction(name: "large_fragment") else { throw Failure(reason: "functions") }
        try need(kernel.functionType == .kernel && vertex.functionType == .vertex && fragment.functionType == .fragment, "stage types")
        let compute = try dev.makeComputePipelineState(function: kernel)
        try need(compute.maxTotalThreadsPerThreadgroup >= 32, "threadgroup capacity")
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = vertex; descriptor.fragmentFunction = fragment
        descriptor.rasterSampleCount = 1
        for i in 0..<2 {
            descriptor.colorAttachments[i].pixelFormat = .rgba32Uint
            descriptor.colorAttachments[i].isBlendingEnabled = false
            descriptor.colorAttachments[i].writeMask = .all
        }
        let render = try dev.makeRenderPipelineState(descriptor: descriptor)
        let converted = try buffer(512)
        guard let cb = queue.makeCommandBuffer(), let c = cb.makeComputeCommandEncoder() else { throw Failure(reason: "compute command") }
        c.setComputePipelineState(compute)
        c.setBuffer(halfInput, offset: 64, index: 0); c.setBuffer(converted, offset: 64, index: 1)
        var params = SIMD2<UInt32>(20, UInt32(modeIndex))
        c.setBytes(&params, length: 8, index: 2)
        c.dispatchThreadgroups(MTLSize(width: 1, height: 1, depth: 1), threadsPerThreadgroup: MTLSize(width: 32, height: 1, depth: 1))
        c.endEncoding()
        var outputs: [(String, MTLBuffer)] = [(mode + "-u16.bin", converted)]
        for caseIndex in 0..<2 {
            let pass = MTLRenderPassDescriptor()
            var textures: [MTLTexture] = []
            for i in 0..<2 {
                let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba32Uint, width: 4, height: 4, mipmapped: false)
                td.storageMode = .private; td.usage = .renderTarget
                guard let texture = dev.makeTexture(descriptor: td) else { throw Failure(reason: "texture") }
                textures.append(texture)
                pass.colorAttachments[i].texture = texture
                pass.colorAttachments[i].loadAction = .clear; pass.colorAttachments[i].storeAction = .store
                pass.colorAttachments[i].clearColor = MTLClearColorMake(0, 0, 0, 0)
            }
            guard let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(reason: "render encoder") }
            e.setRenderPipelineState(render); e.setCullMode(.none); e.setTriangleFillMode(.fill)
            e.setViewport(MTLViewport(originX: 0, originY: 0, width: 4, height: 4, znear: 0, zfar: 1))
            e.setScissorRect(MTLScissorRect(x: 0, y: 0, width: 4, height: 4))
            e.setVertexBuffer(vertices, offset: 64 + caseIndex * 96, index: 0)
            var tag = UInt32((modeIndex << 8) | (caseIndex << 4))
            e.setFragmentBytes(&tag, length: 4, index: 0)
            e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            e.endEncoding()
            guard let blit = cb.makeBlitCommandEncoder() else { throw Failure(reason: "readback encoder") }
            for i in 0..<2 {
                let raw = try buffer(1536)
                blit.copy(from: textures[i], sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                    sourceSize: MTLSize(width: 4, height: 4, depth: 1), to: raw, destinationOffset: 256,
                    destinationBytesPerRow: 256, destinationBytesPerImage: 1024)
                outputs.append(("\(mode)-v\(caseIndex)-\(i).bin", raw))
            }
            blit.endEncoding()
        }
        let completion = DispatchSemaphore(value: 0)
        cb.addCompletedHandler { _ in completion.signal() }
        cb.commit()
        try need(completion.wait(timeout: .now() + 10) == .success, "GPU timeout (no retry)")
        let unchanged = snapshot(halfInput) == halfBefore && snapshot(vertices) == verticesBefore
        receipt += "\(mode)\t\(sha(bytes))\t\(cb.status.rawValue)\t\(cb.error == nil ? 0 : 1)\t\(unchanged ? 1 : 0)\n"
        try need(cb.status == .completed && cb.error == nil && unchanged, "command status or changed input")
        for (name, b) in outputs { try save(name, snapshot(b)) }
    }
    report["status"] = "NATIVE_BYTES_CAPTURED_PENDING_VALIDATION"
    report["command_buffers"] = 2; report["draws"] = 4; report["conversion_dispatches"] = 2
}
var code: Int32 = 0
do { try run() } catch {
    if let e = error as? Failure { report["failure"] = e.reason }
    else { let e = error as NSError; report["failure"] = ["domain": e.domain, "code": e.code] }
    code = 3
}
do {
    try save("gpu.tsv", Data(receipt.utf8))
    try save("gpu.json", JSONSerialization.data(withJSONObject: report, options: [.sortedKeys, .prettyPrinted]))
} catch { code = 4 }
exit(code)
