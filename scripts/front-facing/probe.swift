// Minimal public Metal bridge. Only completed GPU readbacks become *.gpu.bin.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let reason: String }
func need(_ ok: Bool, _ reason: String) throws { if !ok { throw Failure(reason: reason) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
struct Spec: Decodable {
    let id: Int; let mode: String; let order: Int; let vertexOffset: Int
    let tag: UInt32; let skipRow: UInt32
}
struct Manifest: Decodable {
    let schema: Int; let width: Int; let height: Int; let pitch: Int; let guardBytes: Int
    let cases: [Spec]
}
var report: [String: Any] = ["schema": 1, "status": "FAILED_NOT_PASS", "cpu_result_fallback": false]
var receipts = "id\tstatus\terror_present\twinding_call\tvertex_count\tvertex_offset\tgpu_start\tgpu_end\n"

func run(_ source: URL, _ out: URL) throws {
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little endian required")
    let manifestData = try Data(contentsOf: out.appendingPathComponent("cases.json"))
    let vertices = try Data(contentsOf: out.appendingPathComponent("vertices.bin"))
    let seed = try Data(contentsOf: out.appendingPathComponent("seed.bin"))
    let m = try JSONDecoder().decode(Manifest.self, from: manifestData)
    let modes = ["default", "default", "clockwise", "clockwise", "counterClockwise", "counterClockwise", "default"]
    try need(m.schema == 1 && m.width == 10 && m.height == 10 && m.pitch == 256 && m.guardBytes == 256 && m.cases.count == 7 && vertices.count == 96 && seed.count == 3072, "fixed input layout")
    for (i, c) in m.cases.enumerated() {
        try need(c.id == i && c.mode == modes[i] && c.order == i % 2 && c.vertexOffset == (i % 2) * 48 && c.tag == UInt32(65 + i) && c.skipRow == (i == 6 ? 4 : UInt32.max), "fixed capture plan")
    }
    report["input_sha256"] = ["cases.json": sha(manifestData), "vertices.bin": sha(vertices), "seed.bin": sha(seed)]
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    report["toolchain_and_source_receipts"] = "../commands.tsv; ../os.txt; ../xcode.txt; ../sdk-version.txt; ../sdk-build.txt; ../source-sha256.txt"
    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else { throw Failure(reason: "Metal unavailable; no substitute") }
    report["device"] = ["name": dev.name, "registry_id": String(dev.registryID), "unified_memory": dev.hasUnifiedMemory, "low_power": dev.isLowPower, "removable": dev.isRemovable]
    if #available(macOS 14.0, *) { report["device_architecture"] = dev.architecture.name }
    let options = MTLCompileOptions()
    options.languageVersion = .version3_1
    options.fastMathEnabled = false
    let shader = try Data(contentsOf: source.appendingPathComponent("probe.metal"))
    report["msl_sha256"] = sha(shader)
    report["compiler"] = ["language": "Metal 3.1", "language_enum": options.languageVersion.rawValue, "fast_math": false, "route": "MTLDevice.makeLibrary(source:options:)"]
    let library = try dev.makeLibrary(source: String(decoding: shader, as: UTF8.self), options: options)
    let d = MTLRenderPipelineDescriptor()
    d.vertexFunction = library.makeFunction(name: "vertex_bits")
    d.fragmentFunction = library.makeFunction(name: "facing_bit")
    try need(d.vertexFunction != nil && d.fragmentFunction != nil, "shader functions")
    d.rasterSampleCount = 1
    d.isRasterizationEnabled = true
    d.isAlphaToCoverageEnabled = false
    d.isAlphaToOneEnabled = false
    d.colorAttachments[0].pixelFormat = .rgba8Uint
    d.colorAttachments[0].isBlendingEnabled = false
    d.colorAttachments[0].writeMask = .all
    let pso = try dev.makeRenderPipelineState(descriptor: d)
    report["pipeline"] = ["format": "RGBA8Uint", "format_enum": d.colorAttachments[0].pixelFormat.rawValue, "samples": 1, "blending": false, "write_mask": d.colorAttachments[0].writeMask.rawValue, "depth_stencil_attachments": "none"]
    report["geometry"] = ["viewport": [1, 1, 8, 8, 0, 1], "scissor": [1, 1, 8, 8], "cull_mode": "none", "fill": "fill", "vertex_start": 0, "vertex_count": 3, "fresh_encoder_each_case": true, "default_setFrontFacing_call": "omitted", "vertex_buffer_guard_bytes": 64]

    func buffer(_ length: Int) throws -> MTLBuffer {
        guard let b = dev.makeBuffer(length: length, options: .storageModeShared) else { throw Failure(reason: "buffer allocation") }
        memset(b.contents(), 0xcd, length)
        return b
    }
    func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    let vb = try buffer(64 + vertices.count + 64)
    vertices.withUnsafeBytes { vb.contents().advanced(by: 64).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let vbBefore = snapshot(vb)
    let upload = try buffer(seed.count)
    seed.withUnsafeBytes { upload.contents().copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let size = MTLSize(width: 10, height: 10, depth: 1)
    let origin = MTLOrigin(x: 0, y: 0, z: 0)
    var rawHashes: [String: String] = [:]
    for c in m.cases {
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Uint, width: 10, height: 10, mipmapped: false)
        td.storageMode = .private; td.usage = .renderTarget; td.hazardTrackingMode = .tracked
        guard let texture = dev.makeTexture(descriptor: td), let cb = queue.makeCommandBuffer(), let blitIn = cb.makeBlitCommandEncoder() else { throw Failure(reason: "texture/command/upload encoder") }
        try need(texture.pixelFormat == .rgba8Uint && texture.width == 10 && texture.height == 10 && texture.sampleCount == 1 && texture.storageMode == .private, "actual texture properties")
        let raw = try buffer(seed.count)
        cb.label = "front-facing-\(c.id)"
        blitIn.copy(from: upload, sourceOffset: 256, sourceBytesPerRow: 256, sourceBytesPerImage: 2560, sourceSize: size, to: texture, destinationSlice: 0, destinationLevel: 0, destinationOrigin: origin)
        blitIn.endEncoding()
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = texture
        pass.colorAttachments[0].loadAction = .load
        pass.colorAttachments[0].storeAction = .store
        guard let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(reason: "render encoder") }
        e.setRenderPipelineState(pso)
        e.setCullMode(.none)
        e.setTriangleFillMode(.fill)
        e.setViewport(MTLViewport(originX: 1, originY: 1, width: 8, height: 8, znear: 0, zfar: 1))
        e.setScissorRect(MTLScissorRect(x: 1, y: 1, width: 8, height: 8))
        // Crucial: default cases make NO winding setter call on this fresh encoder.
        if c.mode == "clockwise" { e.setFrontFacing(.clockwise) }
        if c.mode == "counterClockwise" { e.setFrontFacing(.counterClockwise) }
        e.setVertexBuffer(vb, offset: 64 + c.vertexOffset, index: 0)
        var params = SIMD2<UInt32>(c.tag, c.skipRow)
        e.setFragmentBytes(&params, length: 8, index: 0)
        e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        e.endEncoding()
        guard let blitOut = cb.makeBlitCommandEncoder() else { throw Failure(reason: "readback encoder") }
        blitOut.copy(from: texture, sourceSlice: 0, sourceLevel: 0, sourceOrigin: origin, sourceSize: size, to: raw, destinationOffset: 256, destinationBytesPerRow: 256, destinationBytesPerImage: 2560)
        blitOut.endEncoding()
        cb.commit()
        cb.waitUntilCompleted()
        let fields = [String(c.id), String(cb.status.rawValue), cb.error == nil ? "0" : "1", c.mode == "default" ? "omitted" : c.mode, "3", String(64 + c.vertexOffset), String(cb.gpuStartTime), String(cb.gpuEndTime)]
        receipts += fields.joined(separator: "\t") + "\n"
        try need(cb.status == .completed && cb.error == nil, "GPU command did not complete without error")
        let bytes = snapshot(raw) // No host modification, conversion, or synthesized color.
        let name = "case-\(c.id).gpu.bin"
        try bytes.write(to: out.appendingPathComponent(name), options: .withoutOverwriting)
        rawHashes[name] = sha(bytes)
        try need(snapshot(vb) == vbBefore && snapshot(upload) == seed, "input bytes/guards changed")
    }
    report["raw_sha256"] = rawHashes
    report["input_buffers_and_guards_unchanged"] = true
    report["command_count"] = 7
    report["gpu_time_note"] = "Zero timestamps, if returned, mean unavailable; no fabricated timings."
    report["status"] = "CAPTURED_PENDING_VALIDATION"
}

guard CommandLine.arguments.count == 3 else { exit(2) }
let out = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
var code: Int32 = 0
do { try run(URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true), out) }
catch {
    if let f = error as? Failure { report["failure"] = f.reason }
    else { let e = error as NSError; report["failure"] = ["domain": e.domain, "code": e.code] }
    code = 3
}
report["exit_code"] = code
do {
    try Data(receipts.utf8).write(to: out.appendingPathComponent("gpu-commands.tsv"), options: .withoutOverwriting)
    let json = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
    try json.write(to: out.appendingPathComponent("gpu.json"), options: .withoutOverwriting)
} catch { code = 4 }
print("native_capture_exit=\(code)")
exit(code)
