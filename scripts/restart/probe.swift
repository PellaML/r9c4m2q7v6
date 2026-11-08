// Only real, completed GPU copies become *.gpu.bin. No source-compilation fallback.
import Foundation
import Metal
import CryptoKit
import Dispatch
struct Failure: Error { let reason: String }
func need(_ ok: Bool, _ reason: String) throws { if !ok { throw Failure(reason: reason) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
struct Draw: Decodable { let primitive: Int; let indexBits: Int; let offset: Int; let count: Int; let instances: Int; let baseInstance: Int }
struct Capture: Decodable { let id: Int; let name: String; let skipRow: UInt32; let requireForeground: Bool; let draws: [Draw] }
struct Manifest: Decodable {
    let schema: Int; let width: Int; let height: Int; let pitch: Int; let guardBytes: Int
    let indexBytes: Int; let captureCount: Int; let captures: [Capture]
}
var report: [String: Any] = ["schema": 1, "status": "FAILED_NOT_PASS", "cpu_result_fallback": false]
var commands = "id\tname\tstatus\terror_present\tdraw_calls\tgpu_start\tgpu_end\n"
var draws = "capture\tdraw\tprimitive\tindex_bits\tindex_offset\tindex_count\tinstances\tbase_instance\tbase_vertex\n"
func run(_ source: URL, _ out: URL) throws {
    func input(_ name: String) throws -> Data {
        let data = try Data(contentsOf: out.appendingPathComponent(name))
        try need(data.count <= 1_048_576, "input byte budget"); return data
    }
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little-endian host")
    let md = try input("cases.json"), positions = try input("vertices.bin"), indices = try input("indices.bin"), seed = try input("seed.bin")
    let m = try JSONDecoder().decode(Manifest.self, from: md)
    try need(m.schema == 1 && m.width == 34 && m.height == 18 && m.pitch == 256 && m.guardBytes == 256 && positions.count == 240 && seed.count == 5120 && indices.count == m.indexBytes && indices.count <= 16384 && m.captureCount == m.captures.count && (1...64).contains(m.captureCount), "fixed input layout/bounds")
    for (i, c) in m.captures.enumerated() {
        try need(c.id == i && c.name.range(of: "^[a-z0-9-]{1,64}$", options: .regularExpression) != nil && c.draws.count <= 16 && (c.skipRow == UInt32.max || c.skipRow == 5), "capture bounds")
        for d in c.draws {
            try need([3, 4].contains(d.primitive) && [16, 32].contains(d.indexBits) && d.offset >= 0 && d.offset <= indices.count && d.offset % 4 == 0 && (1...256).contains(d.count) && (1...2).contains(d.instances) && (0...1).contains(d.baseInstance) && d.instances + d.baseInstance <= 2 && d.offset + d.count * (d.indexBits / 8) <= indices.count, "draw bounds")
            for j in 0..<d.count {
                var id: UInt32 = 0
                for byte in 0..<(d.indexBits / 8) { id |= UInt32(indices[d.offset + j * (d.indexBits / 8) + byte]) << (byte * 8) }
                try need(id <= 14 || id == 65534 || id == 65535, "all indices must be valid shader inputs")
            }
        }
    }
    try need(Set(m.captures.map { $0.name }).count == m.captureCount, "unique captures")
    report["input_sha256"] = ["cases.json": sha(md), "vertices.bin": sha(positions), "indices.bin": sha(indices), "seed.bin": sha(seed)]
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    report["software_receipts"] = "../commands.tsv; ../os.txt; ../os-build.txt; ../xcode.txt; ../sdk-version.txt; ../sdk-build.txt; ../source-sha256.txt"
    guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else { throw Failure(reason: "Metal unavailable; no substitute") }
    report["device"] = ["name": device.name, "registry_id": String(device.registryID), "unified_memory": device.hasUnifiedMemory, "low_power": device.isLowPower, "removable": device.isRemovable]
    if #available(macOS 14.0, *) { report["device_architecture"] = device.architecture.name }
    let air = try input("probe.air"), lib = try input("probe.metallib"), pairData = try input("compile-pair.json")
    let pair = try JSONSerialization.jsonObject(with: pairData) as? [String: Any]
    let shader = try Data(contentsOf: source.appendingPathComponent("probe.metal"))
    try need(!air.isEmpty && lib.prefix(4) == Data([0x4d, 0x54, 0x4c, 0x42]) && pair?["source_sha256"] as? String == sha(shader) && pair?["air_sha256"] as? String == sha(air) && pair?["metallib_sha256"] as? String == sha(lib) && pair?["compile_exit"] as? Int == 0 && pair?["link_exit"] as? Int == 0 && pair?["link_input"] as? String == "probe.air", "exact offline source/AIR/metallib chain")
    let library = try device.makeLibrary(data: lib.withUnsafeBytes { DispatchData(bytes: $0) })
    try need(Set(library.functionNames) == Set(["indexed_vertices", "primitive_bits"]), "offline entry points")
    report["offline_pair"] = pair
    report["loaded_metallib_sha256"] = sha(lib)
    report["source_compile_fallback"] = false
    let pd = MTLRenderPipelineDescriptor()
    pd.vertexFunction = library.makeFunction(name: "indexed_vertices")
    pd.fragmentFunction = library.makeFunction(name: "primitive_bits")
    pd.rasterSampleCount = 1; pd.isAlphaToCoverageEnabled = false; pd.isAlphaToOneEnabled = false
    pd.isRasterizationEnabled = true
    pd.colorAttachments[0].pixelFormat = .rgba8Uint
    pd.colorAttachments[0].isBlendingEnabled = false; pd.colorAttachments[0].writeMask = .all
    let pipeline = try device.makeRenderPipelineState(descriptor: pd)
    try need(MTLPrimitiveType.triangle.rawValue == 3 && MTLPrimitiveType.triangleStrip.rawValue == 4, "primitive enum contract")
    report["state"] = ["format": "RGBA8Uint", "format_enum": pd.colorAttachments[0].pixelFormat.rawValue, "samples": 1, "blending": false, "write_mask": pd.colorAttachments[0].writeMask.rawValue, "depth_stencil": "none", "front_winding": "explicit clockwise", "cull": "none", "viewport": [1, 1, 32, 16, 0, 1], "scissor": [1, 1, 32, 16], "index_type_uint16": MTLIndexType.uint16.rawValue, "index_type_uint32": MTLIndexType.uint32.rawValue, "base_vertex": 0]
    func buffer(_ n: Int) throws -> MTLBuffer {
        guard let b = device.makeBuffer(length: n, options: .storageModeShared) else { throw Failure(reason: "buffer allocation") }
        memset(b.contents(), 0xcd, n); return b
    }
    func upload(_ d: Data) throws -> MTLBuffer {
        let b = try buffer(d.count + 128)
        d.withUnsafeBytes { b.contents().advanced(by: 64).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }; return b
    }
    func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    let vb = try upload(positions), ib = try upload(indices), sb = try buffer(seed.count)
    seed.withUnsafeBytes { sb.contents().copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let vbBefore = snapshot(vb), ibBefore = snapshot(ib)
    let size = MTLSize(width: 34, height: 18, depth: 1), origin = MTLOrigin(x: 0, y: 0, z: 0)
    var hashes: [String: String] = [:]
    for c in m.captures {
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Uint, width: 34, height: 18, mipmapped: false)
        td.storageMode = .private; td.usage = .renderTarget; td.hazardTrackingMode = .tracked
        guard let texture = device.makeTexture(descriptor: td), let cb = queue.makeCommandBuffer(), let copyIn = cb.makeBlitCommandEncoder() else { throw Failure(reason: "texture/command/upload encoder") }
        try need(texture.pixelFormat == .rgba8Uint && texture.width == 34 && texture.height == 18 && texture.sampleCount == 1 && texture.storageMode == .private, "actual texture properties")
        cb.label = c.name
        let raw = try buffer(seed.count)
        copyIn.copy(from: sb, sourceOffset: 256, sourceBytesPerRow: 256, sourceBytesPerImage: 4608, sourceSize: size, to: texture, destinationSlice: 0, destinationLevel: 0, destinationOrigin: origin)
        copyIn.endEncoding()
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = texture; pass.colorAttachments[0].loadAction = .load; pass.colorAttachments[0].storeAction = .store
        guard let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(reason: "render encoder") }
        e.setRenderPipelineState(pipeline); e.setCullMode(.none); e.setTriangleFillMode(.fill); e.setFrontFacing(.clockwise)
        e.setViewport(MTLViewport(originX: 1, originY: 1, width: 32, height: 16, znear: 0, zfar: 1))
        e.setScissorRect(MTLScissorRect(x: 1, y: 1, width: 32, height: 16))
        e.setVertexBuffer(vb, offset: 64, index: 0)
        var skip = c.skipRow; e.setFragmentBytes(&skip, length: 4, index: 0)
        for (j, d) in c.draws.enumerated() {
            let primitive: MTLPrimitiveType = d.primitive == 3 ? .triangle : .triangleStrip
            let kind: MTLIndexType = d.indexBits == 16 ? .uint16 : .uint32
            e.drawIndexedPrimitives(type: primitive, indexCount: d.count, indexType: kind, indexBuffer: ib, indexBufferOffset: 64 + d.offset, instanceCount: d.instances, baseVertex: 0, baseInstance: d.baseInstance)
            let fields = [c.id, j, Int(primitive.rawValue), d.indexBits, 64 + d.offset, d.count, d.instances, d.baseInstance, 0]
            draws += fields.map(String.init).joined(separator: "\t") + "\n"
        }
        e.endEncoding()
        guard let copyOut = cb.makeBlitCommandEncoder() else { throw Failure(reason: "readback encoder") }
        copyOut.copy(from: texture, sourceSlice: 0, sourceLevel: 0, sourceOrigin: origin, sourceSize: size, to: raw, destinationOffset: 256, destinationBytesPerRow: 256, destinationBytesPerImage: 4608)
        copyOut.endEncoding(); cb.commit(); cb.waitUntilCompleted()
        commands += [String(c.id), c.name, String(cb.status.rawValue), cb.error == nil ? "0" : "1", String(c.draws.count), String(cb.gpuStartTime), String(cb.gpuEndTime)].joined(separator: "\t") + "\n"
        try need(cb.status == .completed && cb.error == nil, "GPU command failed")
        let actual = snapshot(raw), name = "case-\(c.id).gpu.bin"
        try actual.write(to: out.appendingPathComponent(name), options: .withoutOverwriting)
        hashes[name] = sha(actual)
        try need(snapshot(vb) == vbBefore && snapshot(ib) == ibBefore && snapshot(sb) == seed, "input bytes/guards changed")
    }
    report["raw_sha256"] = hashes
    report["command_count"] = m.captureCount
    report["input_buffers_and_guards_unchanged"] = true
    report["status"] = "CAPTURED_PENDING_VALIDATION"
}
guard CommandLine.arguments.count == 3 else { exit(2) }
let out = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
var code: Int32 = 0
do { try run(URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true), out) }
catch { if let f = error as? Failure { report["failure"] = f.reason } else { let e = error as NSError; report["failure"] = ["domain": e.domain, "code": e.code] }; code = 3 }
report["exit_code"] = code
do {
    try Data(commands.utf8).write(to: out.appendingPathComponent("gpu-commands.tsv"), options: .withoutOverwriting)
    try Data(draws.utf8).write(to: out.appendingPathComponent("draws.tsv"), options: .withoutOverwriting)
    let json = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
    try json.write(to: out.appendingPathComponent("gpu.json"), options: .withoutOverwriting)
} catch { code = 4 }
print("native_capture_exit=\(code)")
exit(code)
