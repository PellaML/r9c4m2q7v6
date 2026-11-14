// Native Metal API bridge only: no numerical model and no software fallback.
import Foundation
import Metal
import CryptoKit
import Dispatch

struct Failure: Error { let reason: String }
func need(_ value: Bool, _ reason: String) throws {
    if !value { throw Failure(reason: reason) }
}
func sha(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}
func errorInfo(_ error: Error) -> [String: Any] {
    if let f = error as? Failure { return ["reason": f.reason] }
    let e = error as NSError
    var info: [String: Any] = ["domain": e.domain, "code": e.code,
        "description": String(e.localizedDescription.prefix(4096))]
    if let reason = e.localizedFailureReason { info["reason"] = String(reason.prefix(4096)) }
    if let underlying = e.userInfo[NSUnderlyingErrorKey] as? NSError {
        info["underlying"] = ["domain": underlying.domain, "code": underlying.code,
            "description": String(underlying.localizedDescription.prefix(4096))]
    }
    return info
}
struct Row: Decodable {
    let id: String, source_f32: [String], destination_f16: [String]
}
struct Corpus: Decodable { let schema: Int, rows: [Row] }

guard CommandLine.arguments.count == 3,
      ["precise", "fast"].contains(CommandLine.arguments[2]) else { exit(2) }
let mode = CommandLine.arguments[2]
let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let env = ProcessInfo.processInfo.environment
let rows = 64, width = 3, height = 66, pitch = 256, guardBytes = 256
let rawBytes = 17408, sourceBytes = 1024, traceBytes = 3200
var captures: [[String: Any]] = []
var report: [String: Any] = ["schema": 1, "status": "UNQUALIFIED", "cpu_fallback": false,
    "mode": mode, "os": ProcessInfo.processInfo.operatingSystemVersionString,
    "os_build": env["ACTUAL_OS_BUILD"] ?? "unknown", "runner": env["RUNNER_LABEL"] ?? "unknown",
    "reviewed_commit": env["REVIEWED_COMMIT"] ?? "unknown", "device": NSNull(),
    "geometry": ["rows": rows, "width": width, "height": height, "pitch": pitch,
                 "raw_guard_bytes": guardBytes, "source_offset": 64, "trace_offset": 64,
                 "trace_row_bytes": 48, "raw_bytes": rawBytes, "trace_bytes": traceBytes],
    "guard_scope": "host-visible buffers, row padding and untouched texels; not opaque texture allocation padding"]

func checkpoint() throws {
    report["captures"] = captures
    let data = try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys, .prettyPrinted])
    try need(data.count <= 1_048_576, "metadata size bound")
    try data.write(to: root.appendingPathComponent("gpu.json"), options: .atomic)
}
func phase(_ name: String) throws { report["phase"] = name; try checkpoint() }
func read(_ url: URL) throws -> Data {
    let a = try FileManager.default.attributesOfItem(atPath: url.path)
    let length = (a[.size] as? NSNumber)?.intValue ?? Int.max
    try need(a[.type] as? FileAttributeType == .typeRegular && length <= 1_048_576, "input type/size")
    return try Data(contentsOf: url)
}
func save(_ name: String, _ data: Data) throws {
    try need(!name.contains("/") && !name.contains("\\") && data.count <= 1_048_576, "output name/size")
    try data.write(to: root.appendingPathComponent(name), options: .withoutOverwriting)
}
func word(_ text: String, _ digits: Int) throws -> UInt32 {
    try need(text.count == digits && text.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }, "hex word")
    guard let value = UInt32(text, radix: 16) else { throw Failure(reason: "hex decode") }
    return value
}
func put(_ value: UInt32, _ bytes: Int, _ offset: Int, _ data: inout Data) {
    for i in 0..<bytes { data[offset + i] = UInt8(truncatingIfNeeded: value >> (8 * i)) }
}

func run() throws {
    try phase("inputs")
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little endian required")
    let corpusData = try read(root.deletingLastPathComponent().appendingPathComponent("cases.json"))
    let corpus = try JSONDecoder().decode(Corpus.self, from: corpusData)
    try need(corpus.schema == 1 && corpus.rows.count == rows && Set(corpus.rows.map { $0.id }).count == rows,
             "fixed 64-row corpus")
    var inputs = Data(count: sourceBytes)
    var seed = Data(repeating: 0xcd, count: rawBytes)
    // Finite, lane-distinct border texels; active destinations are raw half words.
    for y in 0..<height { for x in 0..<width {
        for (lane, base) in [UInt32(0x3555), 0xb400, 0x0003, 0x3a00].enumerated() {
            put(base + UInt32(y * width + x), 2, guardBytes + y * pitch + x * 8 + lane * 2, &seed)
        }
    } }
    for (i, row) in corpus.rows.enumerated() {
        try need(row.id.count <= 64 && row.source_f32.count == 4 && row.destination_f16.count == 4, "row shape")
        for lane in 0..<4 {
            let s = try word(row.source_f32[lane], 8), d = try word(row.destination_f16[lane], 4)
            let value = Float(bitPattern: s)
            try need(value.isFinite && abs(value) <= 65504 && (d & 0x7c00) != 0x7c00,
                     "finite corpus; no half-conversion infinity")
            put(s, 4, i * 16 + lane * 4, &inputs)
            put(d, 2, guardBytes + (i + 1) * pitch + 8 + lane * 2, &seed)
        }
    }
    try save("source.bin", inputs); try save("seed.bin", seed)
    report["cases_sha256"] = sha(corpusData)
    report["source_sha256"] = sha(inputs); report["seed_sha256"] = sha(seed)
    try phase("device")
    guard let dev = MTLCreateSystemDefaultDevice() else { throw Failure(reason: "Metal device unavailable; no fallback") }
    report["device"] = dev.name; report["unified_memory"] = dev.hasUnifiedMemory
    try phase("queue")
    try need(dev.name == "Apple Paravirtual device" && dev.hasUnifiedMemory, "reference device changed; preserve metadata, stop")
    guard let queue = dev.makeCommandQueue() else { throw Failure(reason: "Metal queue unavailable") }
    try phase("library")
    let libraryBytes = try read(root.appendingPathComponent("program.metallib"))
    try need(Array(libraryBytes.prefix(4)) == [0x4d, 0x54, 0x4c, 0x42], "metallib signature")
    report["loaded_sha256"] = sha(libraryBytes)
    // Hash the exact Data passed to Metal. Never compile an alternate source here.
    let library = try dev.makeLibrary(data: libraryBytes.withUnsafeBytes { DispatchData(bytes: $0) })
    try need(Set(library.functionNames) == Set(["triangle", "emit_f16", "emit_f32"]), "exact entry set")
    guard let vertex = library.makeFunction(name: "triangle"),
          let halfFragment = library.makeFunction(name: "emit_f16"),
          let floatFragment = library.makeFunction(name: "emit_f32") else { throw Failure(reason: "entry unavailable") }
    try need(vertex.functionType == .vertex && halfFragment.functionType == .fragment && floatFragment.functionType == .fragment,
             "entry stage types")
    func buffer(_ length: Int) throws -> MTLBuffer {
        guard let b = dev.makeBuffer(length: length, options: .storageModeShared) else { throw Failure(reason: "buffer allocation") }
        try need(b.storageMode == .shared && b.length == length, "shared buffer state")
        memset(b.contents(), 0xcd, length)
        return b
    }
    func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    let source = try buffer(sourceBytes + 128), upload = try buffer(rawBytes)
    inputs.withUnsafeBytes { source.contents().advanced(by: 64).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    seed.withUnsafeBytes { upload.contents().copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let sourceBefore = snapshot(source), uploadBefore = snapshot(upload)
    let size = MTLSize(width: width, height: height, depth: 1), origin = MTLOrigin(x: 0, y: 0, z: 0)
    let specs: [(String, Int, Bool)] = [("seed", 0, false), ("half-union", 16, false),
        ("half-control", 16, true), ("float-union", 32, false), ("float-control", 32, true)]
    for (index, spec) in specs.enumerated() {
        let (name, bits, wrong) = spec
        let tag = UInt32(index + 1) | (mode == "fast" ? 256 : 0)
        try phase("texture-" + name)
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba16Float, width: width, height: height, mipmapped: false)
        td.storageMode = .private; td.usage = [.renderTarget, .shaderRead]; td.hazardTrackingMode = .tracked
        guard let texture = dev.makeTexture(descriptor: td) else { throw Failure(reason: "RGBA16Float target unavailable; no substitute") }
        let sw = texture.swizzle
        try need(texture.pixelFormat == .rgba16Float && texture.pixelFormat.rawValue == 115 &&
                 texture.storageMode == .private && texture.usage == [.renderTarget, .shaderRead] &&
                 texture.textureType == .type2D && texture.width == width && texture.height == height && texture.depth == 1 &&
                 texture.mipmapLevelCount == 1 && texture.sampleCount == 1 && texture.arrayLength == 1 &&
                 texture.buffer == nil && !texture.isFramebufferOnly && texture.hazardTrackingMode == .tracked &&
                 sw.red == .red && sw.green == .green && sw.blue == .blue && sw.alpha == .alpha, "actual texture state")
        var record: [String: Any] = ["name": name, "tag": tag, "return_bits": bits,
            "negative_control": wrong, "texture": ["format": texture.pixelFormat.rawValue,
                "storage": texture.storageMode.rawValue, "usage": texture.usage.rawValue,
                "width": width, "height": height, "samples": 1, "identity_swizzle": true],
            "draws": bits == 0 ? 0 : 1, "vertices": bits == 0 ? 0 : 3]
        var pipeline: MTLRenderPipelineState? = nil
        if bits != 0 {
            let pd = MTLRenderPipelineDescriptor()
            pd.vertexFunction = vertex; pd.fragmentFunction = bits == 16 ? halfFragment : floatFragment
            pd.rasterSampleCount = 1; pd.isAlphaToCoverageEnabled = false
            pd.isAlphaToOneEnabled = false; pd.isRasterizationEnabled = true
            let a = pd.colorAttachments[0]!
            a.pixelFormat = .rgba16Float; a.writeMask = .all; a.isBlendingEnabled = true
            a.sourceRGBBlendFactor = .one; a.destinationRGBBlendFactor = wrong ? .oneMinusSourceAlpha : .oneMinusSourceColor
            a.rgbBlendOperation = .add
            a.sourceAlphaBlendFactor = .one; a.destinationAlphaBlendFactor = .oneMinusSourceAlpha
            a.alphaBlendOperation = .add
            let tuple = [a.sourceRGBBlendFactor.rawValue, a.destinationRGBBlendFactor.rawValue, a.rgbBlendOperation.rawValue,
                         a.sourceAlphaBlendFactor.rawValue, a.destinationAlphaBlendFactor.rawValue, a.alphaBlendOperation.rawValue]
            try need(tuple == [1, wrong ? 5 : 3, 0, 1, 5, 0] && a.writeMask.rawValue == 15, "blend descriptor")
            record["pipeline_descriptor"] = ["format": a.pixelFormat.rawValue, "blend": tuple,
                "blending_enabled": a.isBlendingEnabled, "write_mask": a.writeMask.rawValue,
                "fragment": bits == 16 ? "emit_f16" : "emit_f32", "samples": pd.rasterSampleCount,
                "alpha_to_coverage": pd.isAlphaToCoverageEnabled, "alpha_to_one": pd.isAlphaToOneEnabled,
                "rasterization": pd.isRasterizationEnabled]
            report["current_capture"] = record
            try phase("pipeline-" + name)
            // These are queried descriptor values, not opaque PSO introspection.
            pipeline = try dev.makeRenderPipelineState(descriptor: pd)
        }
        let raw = try buffer(rawBytes), trace = try buffer(traceBytes)
        guard let cb = queue.makeCommandBuffer(), let up = cb.makeBlitCommandEncoder() else { throw Failure(reason: "upload encoder") }
        cb.label = name
        up.copy(from: upload, sourceOffset: guardBytes, sourceBytesPerRow: pitch, sourceBytesPerImage: pitch * height,
                sourceSize: size, to: texture, destinationSlice: 0, destinationLevel: 0, destinationOrigin: origin)
        up.endEncoding()
        if let pipeline = pipeline {
            let pass = MTLRenderPassDescriptor()
            let a = pass.colorAttachments[0]!
            a.texture = texture; a.loadAction = .load; a.storeAction = .store
            record["render_pass"] = ["load": a.loadAction.rawValue, "store": a.storeAction.rawValue,
                "level": a.level, "slice": a.slice, "depth_plane": a.depthPlane, "resolve": a.resolveTexture != nil]
            guard let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(reason: "render encoder") }
            e.setRenderPipelineState(pipeline); e.setCullMode(.none); e.setTriangleFillMode(.fill)
            e.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(width), height: Double(height), znear: 0, zfar: 1))
            e.setScissorRect(MTLScissorRect(x: 1, y: 1, width: 1, height: rows))
            e.setFragmentBuffer(source, offset: 64, index: 0); e.setFragmentBuffer(trace, offset: 64, index: 1)
            var params = SIMD4<UInt32>(UInt32(rows), tag, 0, 0)
            e.setFragmentBytes(&params, length: 16, index: 2)
            e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            e.endEncoding()
        }
        guard let down = cb.makeBlitCommandEncoder() else { throw Failure(reason: "readback encoder") }
        down.copy(from: texture, sourceSlice: 0, sourceLevel: 0, sourceOrigin: origin, sourceSize: size,
                  to: raw, destinationOffset: guardBytes, destinationBytesPerRow: pitch, destinationBytesPerImage: pitch * height)
        down.endEncoding()
        report["current_capture"] = record
        try phase("submit-" + name)
        let done = DispatchSemaphore(value: 0)
        cb.addCompletedHandler { _ in done.signal() }; cb.commit()
        let completed = done.wait(timeout: .now() + 10) == .success
        record["completed_within_10s"] = completed; record["status"] = cb.status.rawValue
        if let error = cb.error { record["error"] = errorInfo(error) } else { record["error"] = NSNull() }
        captures.append(record); report["current_capture"] = record
        try checkpoint()
        try need(completed && cb.status == .completed && cb.error == nil, "GPU failure/timeout; no retry")
        let unchanged = snapshot(source) == sourceBefore && snapshot(upload) == uploadBefore
        captures[captures.count - 1]["inputs_unchanged"] = unchanged
        try checkpoint()
        try need(unchanged, "source/upload or input guards modified")
        let rawData = snapshot(raw)
        try save(name + ".raw.bin", rawData)
        record["raw_sha256"] = sha(rawData); record["inputs_unchanged"] = unchanged
        record["gpu_start"] = cb.gpuStartTime; record["gpu_end"] = cb.gpuEndTime
        if bits != 0 {
            let traceData = snapshot(trace)
            try save(name + ".trace.bin", traceData); record["trace_sha256"] = sha(traceData)
        }
        captures[captures.count - 1] = record
        try checkpoint()
    }
    try need(captures.count == 5, "capture count")
    report["command_buffers"] = 5; report["draws"] = 4; report["vertices"] = 12
    report["status"] = "NATIVE_BYTES_CAPTURED_NOT_MODEL_VALIDATION"
    try phase("complete")
}
var code: Int32 = 0
do { try run() } catch {
    report["status"] = "UNQUALIFIED"; report["failure"] = errorInfo(error); code = 3
}
do { try checkpoint() } catch {
    fputs("Could not preserve GPU metadata: \(error)\n", stderr); code = 4
}
exit(code)
